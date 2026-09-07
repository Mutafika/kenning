//! kenning — enchudb を **コード探索エンジン** として使う PoC。
//!
//! 動機: コード探索は「多くの属性を AND で重ねて少数に絞る + 参照 edge を辿る」= まさに
//! enchudb の設計点 (`and-bench` の faceted 等値 AND)。Kythe / Glean / SCIP / CodeQL の
//! fact モデルを参考に、最小 schema をこう置く:
//!
//!   file (path / crate / lang / loc)
//!   sym  (name / kind / vis / is_async / is_test / crate / module / container=facet,
//!         file = ref edge)                         ← Symbol (SCIP symbol / Kythe semantic node)
//!   call (caller = ref→sym, callee = tag(単純名), callee_sym = ref→sym(解決先), res = facet)
//!                                                   ← who-calls edge (Kythe ref/call)
//!
//! facet は全部 bucket 化されるので「pub かつ async かつ crate=X の fn」= bucket 交差 = µs。
//! who-calls は解決済みなら callee_sym(eid) の逆引き = 精密、未解決でも callee(単純名) で text
//! fallback。名前解決は 2-pass (pass1=全 sym 登録 → pass2=`Type::`/`mod::` 修飾 or 同名ユニーク
//! で解決、曖昧・外部は推測しない)。indexer は `syn` 2.x (per-file / macro 非展開)。

pub(crate) use enchudb::schema::{Database, Table, Value};
pub(crate) use enchudb::{DbState, Engine, FaultKind};
pub(crate) use enchudb_oplog::EntityId;
pub(crate) use quote::ToTokens;
pub(crate) use std::collections::{HashMap, HashSet};
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::sync::atomic::{AtomicBool, Ordering};
pub(crate) use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
pub(crate) use syn::visit::{self, Visit};

// ── kind / visibility の enum encoding (facet) ──
pub(crate) const K_FN: u32 = 0;
pub(crate) const K_METHOD: u32 = 1;
pub(crate) const K_STRUCT: u32 = 2;
pub(crate) const K_ENUM: u32 = 3;
pub(crate) const K_TRAIT: u32 = 4;
pub(crate) const K_CONST: u32 = 5;
pub(crate) const KIND_NAMES: &[&str] = &["fn", "method", "struct", "enum", "trait", "const"];

pub(crate) const V_PUB: u32 = 0;
pub(crate) const V_CRATE: u32 = 1;
pub(crate) const V_RESTRICTED: u32 = 2;
pub(crate) const V_PRIV: u32 = 3;
pub(crate) const VIS_NAMES: &[&str] = &["pub", "crate", "restricted", "priv"];

pub(crate) const LANG_RUST: u32 = 0;
pub(crate) const LANG_MD: u32 = 1;
pub(crate) const LANG_TOML: u32 = 2;
pub(crate) const LANG_YAML: u32 = 3;
/// container 抽出関数を持たない全テキスト。注釈なしで通す (形式追加が索引追加をブロックしない)。
pub(crate) const LANG_TEXT: u32 = 4;

/// text index に載せるファイルの上限 (bytes)。minified/生成物/データファイルの安全弁。
/// 超えるものは索引せず grep 側に残す — `text` は query 時に本文を読むので、ここが速度の支配項。
pub(crate) const TEXT_MAX_BYTES: u64 = 1 << 20; // 1 MiB
/// binary 判定で嗅ぐ先頭 bytes 数。
pub(crate) const BINARY_SNIFF: usize = 8192;

/// 索引しない生成ファイル。`target/` を枝刈りするのと同じ理由 — 人が書いたものではなく、
/// hit 数が多いので本命 (Cargo.toml の依存行、設計 doc) を limit の外へ押し出す。
/// バージョンを知りたい時は `cargo tree` 等の専用ツールのほうが正確。
pub(crate) const GENERATED_FILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "Gemfile.lock",
    "composer.lock",
    "go.sum",
];

/// index 意味論の版。facet の付け方など「schema は同じでも値の意味が変わる」変更で bump する。
/// 版違いの db は増分 update せず full 再 index (update_with_heal 経由、.scip 再利用) に落とす。
/// v2: crate_ を dir heuristic (crates/<name>/ のみ) → 最寄り祖先 Cargo.toml の package name に変更。
/// v3: sym に doc 列 (doc コメント 1 行目) を追加。
/// v4: sym に end_line 列 (item 終端行) を追加 — `read` が定義本体を切り出す下端。
/// v5: file 表に非 Rust テキスト (md/toml/yml/…) も載せる — `text` の対象が .rs 限定でなくなった。
// v6 (2026-08): enchudb 0.14.4 → 0.25.1。意味論は不変だが、旧 enchudb で作った db を一度焼き直して
// growable lazy commit (実 disk 半減) と新 recovery に乗せるため bump (次クエリで自動 heal、repo あたり ~1s)。
pub(crate) const INDEX_VER: u32 = 20; // 7: dir 表 / 8: node_modules 除外 / 9: macro 内 call / 10: 値渡し参照 / 11: 値参照を式全般へ + sym.trait_impl 列 / 12: 仮引数を束縛として除外 / 13: super::/crate::/self:: を module 相対として解決 / 19: 外部呼び出し (std/dep) を R_EXTERNAL に分離 / 20: 受け手不明の method 呼びを候補どまりに (誤確定の除去)

/// このプロセスで鮮度チェック済みか。parse_opts の auto 経路 (maybe_auto_update / auto-index) が
/// 立てる。open_ro 側の warn_if_stale が同じ stat-walk を繰り返さないため — 非 Rust の大 dir を
/// 抱えた repo (例: ios/android 同梱) では walk がクエリ時間の支配項で、二重取りは丸ごと無駄。
pub(crate) static STALE_CHECKED: AtomicBool = AtomicBool::new(false);
/// query の前座 (自動 full index / 自動 update / heal) では stderr を要点 1 行に畳む。
/// 明示 `kenning index` / `update` は計測用の内訳まで出す。stdout はどちらもデータのみ。
pub(crate) static AUTO_QUIET: AtomicBool = AtomicBool::new(false);
pub(crate) fn quiet() -> bool {
    AUTO_QUIET.load(Ordering::Relaxed)
}

// ── 名前解決の信頼度 (call.res facet) ──
// 推測はしない。曖昧・外部は resolved にせず callee_sym を空にする。
pub(crate) const R_UNRESOLVED: u32 = 0; // 同名の定義が index に無い (外部 crate / std / macro)
pub(crate) const R_UNIQUE: u32 = 1; // 同名定義がちょうど 1 つ → 一意解決
pub(crate) const R_QUALIFIED: u32 = 2; // `Type::` / `mod::` 修飾で 1 つに絞れた
pub(crate) const R_AMBIG: u32 = 3; // 同名定義が複数 → 型不明で絞れず未解決
pub(crate) const R_VALUE: u32 = 4; // 関数を値として渡した参照 (`map(f)`)。呼ぶのは渡した先なので確定はしない
/// `sym_by_symbol` の値がこれ = その SCIP symbol は複数の定義に対応してしまっている (ターゲット間衝突)。
/// 確定に使うと嘘になるので、syn の規準に落とす。
pub(crate) const AMBIGUOUS_SYMBOL: EntityId = 0;

pub(crate) const R_MACRO: u32 = 5; // parse できない macro body を字句走査して見つけた `ident(` (確定させない)
/// 呼び先が **repo の外** と確定 (std / 依存 crate)。SCIP が非 workspace symbol と言ったか、
/// 呼べる同名定義が index に 1 つも無いか。**解決できなかったのではなく解決する物が無い**ので、
/// 解決率の分母から外す。ここを R_UNRESOLVED と混ぜると「率が低い = 精度が低い」に見えてしまう
/// (実際は corpus の外部依存率)。例外はマクロ生成の定義 (syn には見えない)。
pub(crate) const R_EXTERNAL: u32 = 6;
/// `x.f()` の受け手の型は syn には分からない。同名 method が repo に 1 つしか無くても、
/// その呼び出しが std/dep の同名 method (`.next()` / `.len()`) である可能性は消せないので
/// **確定させない** (候補どまり)。`self.f()` だけは受け手 = 現在の impl 型と分かるので確定する。
pub(crate) const R_METHOD: u32 = 7;
pub(crate) const RES_NAMES: &[&str] =
    &["unresolved", "unique", "qualified", "ambiguous", "value-ref", "macro-token", "external", "method-name"];

// ── call.is_method の encoding (受け手をどこまで知っているか) ──
pub(crate) const M_NONE: u32 = 0; // 関数 / path 呼び
pub(crate) const M_RECV: u32 = 1; // x.f() — 受け手の型は不明
pub(crate) const M_SELF: u32 = 2; // self.f() — 受け手は現在の impl 型

/// call 表の解決内訳。`stats` と `bench` のヘッダで同じ数字を出すための共有集計。
pub(crate) struct ResolveStats {
    pub total: usize,
    pub counts: Vec<usize>, // RES_NAMES と同じ並び
}
impl ResolveStats {
    pub fn of(call_t: &Table) -> Self {
        let counts: Vec<usize> =
            (0..RES_NAMES.len()).map(|r| call_t.where_eq("res", r as u32).count().unwrap_or(0)).collect();
        ResolveStats { total: call_t.all().count().unwrap_or(0), counts }
    }
    /// path の部分一致で絞った版。「repo のどの部分なら確定を信じてよいか」は
    /// 全体の率より知りたい事が多い (RA が解析しない test target だけ確定率が落ちる、等)。
    pub fn of_path(call_t: &Table, paths: &HashMap<EntityId, String>, needle: &str) -> Self {
        let mut counts = vec![0usize; RES_NAMES.len()];
        let mut total = 0usize;
        for c in call_t.all().find().unwrap_or_default() {
            let e = call_t.entity(c);
            let hit = match e.get("file") {
                Some(Value::Ref(f)) => paths.get(&f).is_some_and(|p| p.contains(needle)),
                _ => false,
            };
            if hit {
                total += 1;
                let r = num(e.get("res")) as usize;
                if let Some(slot) = counts.get_mut(r) {
                    *slot += 1;
                }
            }
        }
        ResolveStats { total, counts }
    }
    pub fn n(&self, r: u32) -> usize {
        self.counts.get(r as usize).copied().unwrap_or(0)
    }
    /// 確定 = callee_sym を引ける (誤りなし)。
    pub fn confirmed(&self) -> usize {
        self.n(R_UNIQUE) + self.n(R_QUALIFIED)
    }
    /// repo 内を呼んでいる call-site 数 = 全体 - 外部確定。
    pub fn local(&self) -> usize {
        self.total.saturating_sub(self.n(R_EXTERNAL))
    }
    /// **精度として読める率**: repo 内呼び出しのうち確定できた割合。
    pub fn local_pct(&self) -> f64 {
        pct_of(self.confirmed(), self.local())
    }
}

pub(crate) fn pct_of(n: usize, d: usize) -> f64 {
    if d > 0 { n as f64 * 100.0 / d as f64 } else { 0.0 }
}

/// no-occ 診断で「同じ行に occurrence があるか」を見る時に探る列の上限。
pub(crate) const COL_PROBE: u32 = 250;

/// bake 後にこの数だけファイルが変わったら SCIP facts は「古い」扱い (bake 推奨 + refs の偽陰性警告)。
pub(crate) const SCIP_STALE_FILES: u32 = 20;


pub(crate) fn txt(v: Option<Value>) -> String {
    match v {
        Some(Value::Text(s)) => s,
        _ => String::new(),
    }
}
pub(crate) fn num(v: Option<Value>) -> u32 {
    match v {
        Some(Value::Number(n)) => n as u32,
        _ => u32::MAX,
    }
}
pub(crate) fn ref_of(v: Option<Value>) -> EntityId {
    match v {
        Some(Value::Ref(e)) => e,
        _ => 0,
    }
}
pub(crate) fn kind_name(k: u32) -> &'static str {
    KIND_NAMES.get(k as usize).copied().unwrap_or("?")
}
pub(crate) fn vis_name(v: u32) -> &'static str {
    VIS_NAMES.get(v as usize).copied().unwrap_or("?")
}
/// facet 文字列 → コード。未知は None (呼び出し側で無視)。
pub(crate) fn kind_code(s: &str) -> Option<u32> {
    KIND_NAMES.iter().position(|k| *k == s).map(|i| i as u32)
}
pub(crate) fn vis_code(s: &str) -> Option<u32> {
    match s {
        "private" => Some(V_PRIV),
        _ => VIS_NAMES.iter().position(|k| *k == s).map(|i| i as u32),
    }
}
pub(crate) fn bool01(s: &str) -> u32 {
    matches!(s, "1" | "true" | "yes" | "y") as u32
}

// ── 責務ごとの module 分割。相互参照は root の再 export 経由 (各 module 冒頭の `use super::*`) ──
mod parse;
mod scip_facts;
mod paths;
mod index;
mod update;
mod bake;
mod query;
mod graph;
mod bench;
mod cache;

pub(crate) use parse::*;
pub(crate) use scip_facts::*;
pub(crate) use paths::*;
pub(crate) use index::*;
pub(crate) use update::*;
pub(crate) use bake::*;
pub(crate) use query::*;
pub(crate) use graph::*;
pub(crate) use bench::*;
pub(crate) use cache::*;

#[cfg(test)]
mod tests;
