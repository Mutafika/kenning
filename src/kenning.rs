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

use enchudb::schema::{Database, Table, Value};
use enchudb_oplog::EntityId;
use quote::ToTokens;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use syn::visit::{self, Visit};

// ── kind / visibility の enum encoding (facet) ──
const K_FN: u32 = 0;
const K_METHOD: u32 = 1;
const K_STRUCT: u32 = 2;
const K_ENUM: u32 = 3;
const K_TRAIT: u32 = 4;
const K_CONST: u32 = 5;
const KIND_NAMES: &[&str] = &["fn", "method", "struct", "enum", "trait", "const"];

const V_PUB: u32 = 0;
const V_CRATE: u32 = 1;
const V_RESTRICTED: u32 = 2;
const V_PRIV: u32 = 3;
const VIS_NAMES: &[&str] = &["pub", "crate", "restricted", "priv"];

const LANG_RUST: u32 = 0;

/// index 意味論の版。facet の付け方など「schema は同じでも値の意味が変わる」変更で bump する。
/// 版違いの db は増分 update せず full 再 index (update_with_heal 経由、.scip 再利用) に落とす。
/// v2: crate_ を dir heuristic (crates/<name>/ のみ) → 最寄り祖先 Cargo.toml の package name に変更。
/// v3: sym に doc 列 (doc コメント 1 行目) を追加。
/// v4: sym に end_line 列 (item 終端行) を追加 — `read` が定義本体を切り出す下端。
const INDEX_VER: u32 = 4;

/// このプロセスで鮮度チェック済みか。parse_opts の auto 経路 (maybe_auto_update / auto-index) が
/// 立てる。open_ro 側の warn_if_stale が同じ stat-walk を繰り返さないため — 非 Rust の大 dir を
/// 抱えた repo (例: ios/android 同梱) では walk がクエリ時間の支配項で、二重取りは丸ごと無駄。
static STALE_CHECKED: AtomicBool = AtomicBool::new(false);

// ── 名前解決の信頼度 (call.res facet) ──
// 推測はしない。曖昧・外部は resolved にせず callee_sym を空にする。
const R_UNRESOLVED: u32 = 0; // 同名の定義が index に無い (外部 crate / std / macro)
const R_UNIQUE: u32 = 1; // 同名定義がちょうど 1 つ → 一意解決
const R_QUALIFIED: u32 = 2; // `Type::` / `mod::` 修飾で 1 つに絞れた
const R_AMBIG: u32 = 3; // 同名定義が複数 → 型不明で絞れず未解決
const RES_NAMES: &[&str] = &["unresolved", "unique", "qualified", "ambiguous"];

/// enchudb oplog (checkpoint 型 ring) の予約容量。
const OPLOG_CAPACITY: usize = 256 * 1024 * 1024;

// ─────────────────────────── indexer (syn) ───────────────────────────

fn classify_vis(v: &syn::Visibility) -> u32 {
    match v {
        syn::Visibility::Public(_) => V_PUB,
        syn::Visibility::Inherited => V_PRIV,
        syn::Visibility::Restricted(r) => {
            if r.in_token.is_none() && r.path.is_ident("crate") {
                V_CRATE
            } else {
                V_RESTRICTED
            }
        }
    }
}

fn is_test_attrs(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path()
            .segments
            .last()
            .is_some_and(|s| s.ident == "test") // #[test] / #[tokio::test] / #[…::test]
    })
}

fn is_cfg_test(attr: &syn::Attribute) -> bool {
    if !attr.path().is_ident("cfg") {
        return false;
    }
    // #[cfg(test)] / #[cfg(all(test, ...))] を雑に判定 (PoC 割り切り)
    attr.to_token_stream().to_string().contains("test")
}

fn line_of(span: proc_macro2::Span) -> u32 {
    span.start().line as u32 // proc-macro2: line は 1-indexed
}
fn col_of(span: proc_macro2::Span) -> u32 {
    span.start().column as u32 // proc-macro2: column は char 単位・0-indexed
}
/// item 全体 (body 含む) の終端行。`read` が定義本体を切り出す範囲の下端。
fn end_line_of<T: syn::spanned::Spanned>(t: &T) -> u32 {
    t.span().end().line as u32
}

/// SCIP 列(UTF-8 バイトオフセット) → proc-macro2 と同じ char 単位の列に変換。
/// rust-analyzer の SCIP は position_encoding=UTF8(バイト)だが syn は char 数を返すので、
/// 非 ASCII 行(enchudb は日本語コメント多数)では両者がズレて位置 join が黙って外れる。
/// ASCII 行は byte==char なので即返し(実測 occurrence の 97% はこの fast path)。
fn byte_col_to_char(line: &str, byte_col: u32) -> u32 {
    if line.is_ascii() {
        return byte_col;
    }
    let bc = byte_col as usize;
    let mut acc = 0usize;
    for (ci, ch) in line.chars().enumerate() {
        if acc >= bc {
            return ci as u32;
        }
        acc += ch.len_utf8();
    }
    line.chars().count() as u32
}

/// 絶対パスを index root からの相対パスに (SCIP の relative_path と突き合わせる鍵)。
fn rel_of(abs: &str, root: &str) -> String {
    let r = root.trim_end_matches('/');
    abs.strip_prefix(r).map(|s| s.trim_start_matches('/').to_string()).unwrap_or_else(|| abs.to_string())
}

// ─────────────────────────── SCIP (rust-analyzer の正確 facts) ───────────────────────────

/// 1 occurrence (参照点)。line/col は SCIP 準拠で 0-indexed。
struct ScipOcc {
    rel_path: String,
    line0: u32,
    col0: u32,
    symbol: String,
    roles: i32, // bit1=Definition, 2=Import, 4=WriteAccess, 8=ReadAccess
}

/// SCIP index を読み、全 occurrence を保持 + 位置 → occurrence index の map を作る。
/// 位置 join の鍵は (relative_path, line0, col0)。
struct Scip {
    occ: Vec<ScipOcc>,
    pos2idx: HashMap<(String, u32, u32), usize>,
    doc_paths: HashSet<String>, // SCIP が解析した doc の rel_path 集合 (no-occ 診断用)
}
impl Scip {
    /// `root` = index 対象ディレクトリ。SCIP のバイト列を char 列に直すのに元ソースを読む。
    fn load(path: &str, root: &str) -> Scip {
        use protobuf::Message;
        let bytes = std::fs::read(path).expect("read .scip");
        let idx = scip::types::Index::parse_from_bytes(&bytes).expect("decode .scip");
        let root = root.trim_end_matches('/');
        let mut occ = Vec::new();
        let mut pos2idx = HashMap::new();
        let mut doc_paths = HashSet::new();
        for doc in &idx.documents {
            doc_paths.insert(doc.relative_path.clone());
            // doc の元ソースを 1 度だけ読み、行→char 列変換に使う(読めなければ byte 列のまま)。
            let src_lines: Option<Vec<String>> = std::fs::read_to_string(format!("{}/{}", root, doc.relative_path))
                .ok()
                .map(|s| s.lines().map(str::to_string).collect());
            for o in &doc.occurrences {
                if o.range.len() >= 2 {
                    let line0 = o.range[0] as u32;
                    let byte_col = o.range[1] as u32;
                    // SCIP の UTF-8 バイト列を syn と同じ char 列へ(非 ASCII 行対策)。
                    let col0 = src_lines
                        .as_ref()
                        .and_then(|ls| ls.get(line0 as usize))
                        .map(|l| byte_col_to_char(l, byte_col))
                        .unwrap_or(byte_col);
                    pos2idx.insert((doc.relative_path.clone(), line0, col0), occ.len());
                    occ.push(ScipOcc {
                        rel_path: doc.relative_path.clone(),
                        line0,
                        col0,
                        symbol: o.symbol.clone(),
                        roles: o.symbol_roles,
                    });
                }
            }
        }
        Scip { occ, pos2idx, doc_paths }
    }
    /// syn の (rel_path, line 1-indexed, col 0-indexed) を SCIP 鍵に変換して symbol を引く。
    fn symbol_at(&self, rel_path: &str, line1: u32, col0: u32) -> Option<&str> {
        self.pos2idx
            .get(&(rel_path.to_string(), line1.saturating_sub(1), col0))
            .map(|&i| self.occ[i].symbol.as_str())
    }
    /// その rel_path を SCIP が解析したか (no-occ が「doc 欠落」か「位置ズレ」かの切り分け)。
    fn has_doc(&self, rel_path: &str) -> bool {
        self.doc_paths.contains(rel_path)
    }
}

/// ファイル内容の 32bit fingerprint (FNV-1a)。増分 index の変更検知用。
/// number 列は u32::MAX を sentinel に使うので、その値だけ避ける。
fn hash_u32(s: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    if h == u32::MAX { h ^ 1 } else { h }
}

/// repo root を推定: 最寄りの `.git` を持つ祖先、無ければ最上位の `Cargo.toml` を持つ祖先。
/// どちらも無ければ None (= 誤爆防止で自動 index しない)。儀式ゼロ (P-A) の要。
fn repo_root_of(dir: &str) -> Option<std::path::PathBuf> {
    let start = std::fs::canonicalize(dir).ok()?;
    let mut topmost_cargo: Option<std::path::PathBuf> = None;
    let mut cur = Some(start.as_path());
    while let Some(p) = cur {
        if p.join(".git").exists() {
            return Some(p.to_path_buf()); // 最寄りの git repo が最優先
        }
        if p.join("Cargo.toml").exists() {
            topmost_cargo = Some(p.to_path_buf()); // 上へ行くほど上書き = 最上位が残る
        }
        cur = p.parent();
    }
    topmost_cargo
}

/// repo root → 自動 db パス (`~/.cache/kenning/<name>-<hash8>.db`)。db 管理を意識させない。
fn auto_db_path(root: &std::path::Path) -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let cache = std::path::Path::new(&home).join(".cache/kenning");
    std::fs::create_dir_all(&cache).ok()?;
    let root_s = root.to_string_lossy();
    let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "repo".into());
    Some(cache.join(format!("{}-{:08x}.db", name, hash_u32(&root_s))).to_string_lossy().to_string())
}

/// dir の db パスを解決: env KENNING_DB > repo root からの自動導出。main.rs の index/update 用。
pub fn default_db_for(dir: &str) -> Option<String> {
    if let Ok(v) = std::env::var("KENNING_DB") {
        if !v.is_empty() {
            return Some(v);
        }
    }
    repo_root_of(dir).and_then(|r| auto_db_path(&r))
}

/// dir の repo root (文字列)。main.rs の `update` (引数なし) 用。
pub fn repo_root_str(dir: &str) -> Option<String> {
    repo_root_of(dir).map(|p| p.to_string_lossy().to_string())
}

/// Cargo.toml の `[package] name` を読む (簡易 parse、toml crate 依存なし)。
/// virtual workspace manifest ([workspace] だけ) は None。
fn pkg_name_of(cargo_toml: &Path) -> Option<String> {
    let s = std::fs::read_to_string(cargo_toml).ok()?;
    let mut in_pkg = false;
    for line in s.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_pkg = t == "[package]";
            continue;
        }
        if in_pkg {
            if let Some(v) = t.strip_prefix("name").and_then(|r| r.trim_start().strip_prefix('=')) {
                let v = v.trim().trim_matches('"');
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// path から crate 名を解決: 最寄り祖先 Cargo.toml の package name (見つからなければ "root")。
/// workspace の crates/ 配置も root+サブ dir 配置 (ルート crate + ffi/ reader/ 等のサブ crate) も同じ論理。
/// cache は「file の親 dir → crate 名」(同 dir のファイルで stat/read を繰り返さない)。
fn crate_of(path: &str, cache: &mut HashMap<PathBuf, String>) -> String {
    let Some(start) = Path::new(path).parent().map(|d| d.to_path_buf()) else {
        return "root".to_string();
    };
    if let Some(n) = cache.get(&start) {
        return n.clone();
    }
    let mut d = start.clone();
    let name = loop {
        if let Some(n) = pkg_name_of(&d.join("Cargo.toml")) {
            break n;
        }
        if !d.pop() {
            break "root".to_string();
        }
    };
    cache.insert(start, name.clone());
    name
}

/// impl の self_ty を基底型名に正規化 ("Foo < T >" → "Foo")。
fn clean_type(ty: &syn::Type) -> String {
    ty.to_token_stream()
        .to_string()
        .split('<')
        .next()
        .unwrap_or("")
        .replace(' ', "")
}

/// 1 呼び出し箇所。名前解決の材料として修飾 (`Type::` / `mod::`) も持つ。
struct RawCall {
    name: String,               // 呼び先の単純名 (path 末尾 / method 名)
    qualifier: Option<String>,  // path の末尾手前 seg (`Engine::new` の "Engine" / `Self`)。method 呼びは None
    is_method: bool,            // x.foo() 形式か
    line: u32,                  // callee ident の行 (1-indexed)
    col: u32,                   // callee ident の列 (0-indexed)。SCIP join の鍵
}

/// 1 関数ボディ内の呼び出し箇所を集める。
#[derive(Default)]
struct CallCollector {
    calls: Vec<RawCall>,
}
impl<'ast> Visit<'ast> for CallCollector {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = &*node.func {
            let segs = &p.path.segments;
            if let Some(last) = segs.last() {
                // 末尾手前 seg を修飾ヒントに (Type / module)。単一 seg なら None。
                let qualifier = if segs.len() >= 2 {
                    Some(segs[segs.len() - 2].ident.to_string())
                } else {
                    None
                };
                self.calls.push(RawCall {
                    name: last.ident.to_string(),
                    qualifier,
                    is_method: false,
                    line: line_of(last.ident.span()),
                    col: col_of(last.ident.span()),
                });
            }
        }
        visit::visit_expr_call(self, node);
    }
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        // 受け手の型は syn だけでは不明 → qualifier なし。同名ユニーク時のみ解決。
        self.calls.push(RawCall {
            name: node.method.to_string(),
            qualifier: None,
            is_method: true,
            line: line_of(node.method.span()),
            col: col_of(node.method.span()),
        });
        visit::visit_expr_method_call(self, node);
    }
}

/// pass1 で作る in-memory シンボル表の 1 定義。名前解決の突き合わせ先。
struct SymDef {
    eid: EntityId,
    kind: u32,         // method 呼び (x.foo()) は method 定義のみを対象にするため
    container: String, // impl 型 (method) / "" (free fn)。`Type::` 修飾の突き合わせ用
    module: String,    // "a::b"。`mod::` 修飾の突き合わせ用
}

/// pass2 まで持ち越す 1 呼び出し箇所 (caller は確定、callee はこれから解決)。
struct CallSite {
    caller: EntityId,
    caller_container: String, // `Self::` 修飾を現在の impl 型に解決するため
    file: EntityId,
    rel_path: String, // SCIP join の鍵 (index root からの相対)
    name: String,
    qualifier: Option<String>,
    is_method: bool,
    line: u32, // callee ident 行 (1-indexed)
    col: u32,  // callee ident 列 (0-indexed)
}

/// 1 つの `impl Trait for Type`。go-to-implementation 用の edge。
struct ImplEdge {
    trait_name: String,
    type_name: String,
    file: EntityId,
    line: u32,
}

/// pass1/pass2 をまたいで貯める累積器 (ファイル横断)。
#[derive(Default)]
struct Acc {
    defs: HashMap<String, Vec<SymDef>>, // name → 定義群 (syn 解決用)
    pending: Vec<CallSite>,             // 未解決 call-site
    impls: Vec<ImplEdge>,               // impl Trait for Type edge
    scip: Option<Scip>,                 // Some なら SCIP 位置 join で正確解決
    sym_by_symbol: HashMap<String, EntityId>, // SCIP symbol 文字列 → 自 index の sym eid
    file_by_rel: HashMap<String, EntityId>,   // rel_path → file eid (SCIP ref-ingest 用)
    crate_cache: HashMap<PathBuf, String>,    // file の親 dir → crate 名 (crate_of の memo)
}

/// 木を歩く間の可変状態。
struct Ctx {
    file_eid: EntityId,
    rel_path: String, // index root からの相対パス (SCIP join 用)
    crate_name: String,
    module: Vec<String>,
    in_test: bool,
    container: String, // 現在の impl 型 (method の所属)
    n_sym: u64,
}

/// fn シグネチャを 1 行文字列に (hover 相当)。token stream の機械的な空白を詰める。
fn sig_text(sig: &syn::Signature) -> String {
    let mut s = sig.to_token_stream().to_string();
    for (from, to) in [
        (" :: ", "::"), (" : ", ": "), (" < ", "<"), ("< ", "<"), (" >", ">"),
        (" ,", ","), (" (", "("), ("( ", "("), (" )", ")"), ("& ", "&"), (" ;", ";"), ("' ", "'"),
    ] {
        s = s.replace(from, to);
    }
    s
}

#[allow(clippy::too_many_arguments)]
fn record_symbol(
    sym_t: &Table,
    ctx: &mut Ctx,
    acc: &mut Acc,
    name: &str,
    kind: u32,
    vis: u32,
    is_async: bool,
    is_test: bool,
    line: u32,
    col: u32,
    end_line: u32,
    sig: Option<&syn::Signature>,
    body: Option<&syn::Block>,
    doc: &str,
) {
    let module = ctx.module.join("::");
    // SCIP があれば、この定義の位置に対応するグローバル symbol 文字列を引いて焼き込む。
    let symbol = acc
        .scip
        .as_ref()
        .and_then(|s| s.symbol_at(&ctx.rel_path, line, col))
        .map(str::to_string)
        .unwrap_or_default();
    let sym_eid = sym_t
        .insert()
        .set("name", name)
        .set("kind", kind)
        .set("vis", vis)
        .set("is_async", is_async as u32)
        .set("is_test", is_test as u32)
        .set("file", Value::Ref(ctx.file_eid))
        .set("module", module.clone())
        .set("crate_", ctx.crate_name.clone())
        .set("container", ctx.container.clone())
        .set("symbol", symbol.as_str())
        .set("sig", sig.map(sig_text).unwrap_or_default().as_str())
        .set("doc", doc)
        .set("line", line)
        .set("end_line", end_line)
        .commit()
        .unwrap();
    ctx.n_sym += 1;
    // 名前解決の突き合わせ先として登録 (syn 用)。
    acc.defs.entry(name.to_string()).or_default().push(SymDef {
        eid: sym_eid,
        kind,
        container: ctx.container.clone(),
        module,
    });
    // SCIP symbol → 自 index の eid (call の正確解決に使う)。
    if !symbol.is_empty() {
        acc.sym_by_symbol.insert(symbol, sym_eid);
    }
    // 呼び出し箇所は全 sym 確定後に解決するので pass2 へ持ち越す。
    if let Some(b) = body {
        let mut cc = CallCollector::default();
        cc.visit_block(b);
        for rc in cc.calls {
            acc.pending.push(CallSite {
                caller: sym_eid,
                caller_container: ctx.container.clone(),
                file: ctx.file_eid,
                rel_path: ctx.rel_path.clone(),
                name: rc.name,
                qualifier: rc.qualifier,
                is_method: rc.is_method,
                line: rc.line,
                col: rc.col,
            });
        }
    }
}

/// attrs の doc コメント 1 行目 (`///` 由来の `#[doc = "…"]` の最初の非空行)。無ければ ""。
/// def/outline で sig と並べて出す用なので 100 char で切る。
fn first_doc_line(attrs: &[syn::Attribute]) -> String {
    for a in attrs {
        if !a.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &a.meta {
            if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
                let t = s.value().trim().to_string();
                if !t.is_empty() {
                    return t.chars().take(100).collect();
                }
            }
        }
    }
    String::new()
}

fn walk_items(items: &[syn::Item], ctx: &mut Ctx, acc: &mut Acc, sym_t: &Table) {
    for it in items {
        walk_item(it, ctx, acc, sym_t);
    }
}

fn walk_item(it: &syn::Item, ctx: &mut Ctx, acc: &mut Acc, sym_t: &Table) {
    match it {
        syn::Item::Fn(f) => {
            let is_test = ctx.in_test || is_test_attrs(&f.attrs);
            record_symbol(
                sym_t,
                ctx,
                acc,
                &f.sig.ident.to_string(),
                K_FN,
                classify_vis(&f.vis),
                f.sig.asyncness.is_some(),
                is_test,
                line_of(f.sig.ident.span()),
                col_of(f.sig.ident.span()),
                end_line_of(f),
                Some(&f.sig),
                Some(&f.block),
                &first_doc_line(&f.attrs),
            );
        }
        syn::Item::Struct(s) => record_symbol(
            sym_t, ctx, acc, &s.ident.to_string(), K_STRUCT,
            classify_vis(&s.vis), false, ctx.in_test, line_of(s.ident.span()), col_of(s.ident.span()), end_line_of(s), None, None,
            &first_doc_line(&s.attrs),
        ),
        syn::Item::Enum(e) => record_symbol(
            sym_t, ctx, acc, &e.ident.to_string(), K_ENUM,
            classify_vis(&e.vis), false, ctx.in_test, line_of(e.ident.span()), col_of(e.ident.span()), end_line_of(e), None, None,
            &first_doc_line(&e.attrs),
        ),
        syn::Item::Const(c) => record_symbol(
            sym_t, ctx, acc, &c.ident.to_string(), K_CONST,
            classify_vis(&c.vis), false, ctx.in_test, line_of(c.ident.span()), col_of(c.ident.span()), end_line_of(c), None, None,
            &first_doc_line(&c.attrs),
        ),
        syn::Item::Trait(t) => {
            record_symbol(
                sym_t, ctx, acc, &t.ident.to_string(), K_TRAIT,
                classify_vis(&t.vis), false, ctx.in_test, line_of(t.ident.span()), col_of(t.ident.span()), end_line_of(t), None, None,
                &first_doc_line(&t.attrs),
            );
            // trait method の container は trait 名 (= `callers put BlobStore` で引けるように)。
            let prev = std::mem::replace(&mut ctx.container, t.ident.to_string());
            for ti in &t.items {
                if let syn::TraitItem::Fn(m) = ti {
                    let is_test = ctx.in_test || is_test_attrs(&m.attrs);
                    record_symbol(
                        sym_t, ctx, acc, &m.sig.ident.to_string(), K_METHOD,
                        V_PUB, m.sig.asyncness.is_some(), is_test,
                        line_of(m.sig.ident.span()), col_of(m.sig.ident.span()), end_line_of(m), Some(&m.sig), m.default.as_ref(),
                        &first_doc_line(&m.attrs),
                    );
                }
            }
            ctx.container = prev;
        }
        syn::Item::Impl(i) => {
            let type_name = clean_type(&i.self_ty);
            // `impl Trait for Type` なら impl edge を記録 (go-to-implementation 用)。
            // trait 名 = path 末尾 seg、line も同 seg の Ident span (Type の span は trait 不要で避ける)。
            if let Some((_, tpath, _)) = &i.trait_ {
                if let Some(seg) = tpath.segments.last() {
                    acc.impls.push(ImplEdge {
                        trait_name: seg.ident.to_string(),
                        type_name: type_name.clone(),
                        file: ctx.file_eid,
                        line: line_of(seg.ident.span()),
                    });
                }
            }
            let prev = std::mem::replace(&mut ctx.container, type_name);
            for ii in &i.items {
                if let syn::ImplItem::Fn(m) = ii {
                    let is_test = ctx.in_test || is_test_attrs(&m.attrs);
                    record_symbol(
                        sym_t, ctx, acc, &m.sig.ident.to_string(), K_METHOD,
                        classify_vis(&m.vis), m.sig.asyncness.is_some(), is_test,
                        line_of(m.sig.ident.span()), col_of(m.sig.ident.span()), end_line_of(m), Some(&m.sig), Some(&m.block),
                        &first_doc_line(&m.attrs),
                    );
                }
            }
            ctx.container = prev;
        }
        syn::Item::Mod(m) => {
            if let Some((_, inner)) = &m.content {
                let test = ctx.in_test || m.attrs.iter().any(is_cfg_test);
                ctx.module.push(m.ident.to_string());
                let prev = std::mem::replace(&mut ctx.in_test, test);
                walk_items(inner, ctx, acc, sym_t);
                ctx.in_test = prev;
                ctx.module.pop();
            }
        }
        _ => {}
    }
}

// ─────────────────────────── 名前解決 (pass2) ───────────────────────────

/// 1 呼び出し箇所を定義表に突き合わせて (解決先 eid, 信頼度) を返す。推測はしない。
fn resolve_call(cs: &CallSite, defs: &HashMap<String, Vec<SymDef>>) -> (Option<EntityId>, u32) {
    let Some(all_cands) = defs.get(&cs.name) else {
        return (None, R_UNRESOLVED); // 同名定義なし = 外部 crate / std / macro
    };
    // method 呼び (x.foo()) の対象は method 定義のみ。free fn `foo()` への誤解決を防ぐ。
    let cands: Vec<&SymDef> = if cs.is_method {
        all_cands.iter().filter(|d| d.kind == K_METHOD).collect()
    } else {
        all_cands.iter().collect()
    };
    if cands.is_empty() {
        return (None, R_UNRESOLVED);
    }
    // 修飾あり: `Type::` / `Self::` / `mod::` で 1 つに絞れるかを試す。
    if let Some(q) = &cs.qualifier {
        // `Self::` は現在の impl 型に読み替える。
        let target = if q == "Self" { cs.caller_container.as_str() } else { q.as_str() };
        // (a) container 一致 = Type の associated fn / method。
        let by_container: Vec<&SymDef> = cands.iter().copied().filter(|d| d.container == target).collect();
        if by_container.len() == 1 {
            return (Some(by_container[0].eid), R_QUALIFIED);
        }
        if by_container.len() > 1 {
            return (None, R_AMBIG); // 同名 Type が別 crate に複数 → 絞れない
        }
        // (b) module 末尾 seg 一致 = `mod::fn`。
        let by_module: Vec<&SymDef> = cands
            .iter()
            .copied()
            .filter(|d| d.module.rsplit("::").next() == Some(target))
            .collect();
        if by_module.len() == 1 {
            return (Some(by_module[0].eid), R_QUALIFIED);
        }
        // 修飾があるのに index 内で一致しない = 外部型/モジュール。単純名 fallback はしない
        // (`HashMap::new` を自前の `Foo::new` に誤解決しないため)。
        return (None, R_UNRESOLVED);
    }
    // 修飾なし (bare fn / method 呼び): 同名がちょうど 1 つなら一意解決、複数なら諦める。
    if cands.len() == 1 {
        (Some(cands[0].eid), R_UNIQUE)
    } else {
        (None, R_AMBIG)
    }
}

/// 解決済み (target, res) で call 行を挿入。解決した call だけ callee_sym を set。
fn do_insert_call(call_t: &Table, cs: &CallSite, target: Option<EntityId>, res: u32) {
    let mut ins = call_t
        .insert()
        .set("caller", Value::Ref(cs.caller))
        .set("callee", cs.name.as_str())
        .set("res", res)
        .set("qual", cs.qualifier.as_deref().unwrap_or(""))
        .set("is_method", cs.is_method as u32)
        .set("file", Value::Ref(cs.file))
        .set("line", cs.line);
    if let Some(eid) = target {
        ins = ins.set("callee_sym", Value::Ref(eid));
    }
    ins.commit().unwrap();
}

/// syn ヒューリスティックで callee を解決して挿入。res を返す。
fn insert_call(call_t: &Table, cs: &CallSite, defs: &HashMap<String, Vec<SymDef>>) -> u32 {
    let (target, res) = resolve_call(cs, defs);
    do_insert_call(call_t, cs, target, res);
    res
}

/// 全 sym 行を走査して `name → [定義]` 表を作る (再パース不要)。増分の再解決で使う。
fn build_defs_from_table(sym_t: &Table) -> HashMap<String, Vec<SymDef>> {
    let mut defs: HashMap<String, Vec<SymDef>> = HashMap::new();
    for e in sym_t.all().find().unwrap() {
        let er = sym_t.entity(e);
        defs.entry(txt(er.get("name"))).or_default().push(SymDef {
            eid: e,
            kind: num(er.get("kind")),
            container: txt(er.get("container")),
            module: txt(er.get("module")),
        });
    }
    defs
}

/// 1 ファイルの sym / call / impl / file 行を全消去。消えた sym の名前を `affected` に集める
/// (他ファイルからの incoming edge を後で再解決するため)。impl_t は旧 index で無いこともある。
fn purge_file(sym_t: &Table, call_t: &Table, file_t: &Table, impl_t: Option<&Table>, file_eid: EntityId, affected: &mut HashSet<String>) {
    for s in sym_t.all().where_ref("file", file_eid).find().unwrap() {
        affected.insert(txt(sym_t.entity(s).get("name")));
        sym_t.entity(s).delete().unwrap();
    }
    for c in call_t.all().where_ref("file", file_eid).find().unwrap() {
        call_t.entity(c).delete().unwrap();
    }
    if let Some(it) = impl_t {
        for ie in it.all().where_ref("file", file_eid).find().unwrap() {
            it.entity(ie).delete().unwrap();
        }
    }
    file_t.entity(file_eid).delete().unwrap();
}

/// 1 ファイルを parse して file 行 + sym 行を挿入し、call-site を `acc.pending` へ持ち越す。
/// parse 失敗なら false (呼び出し側で skip カウント)。
fn index_one_file(file_t: &Table, sym_t: &Table, acc: &mut Acc, root: &str, path_s: &str, src: &str) -> bool {
    let file = match syn::parse_file(src) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let crate_name = crate_of(path_s, &mut acc.crate_cache);
    let loc = src.lines().count() as u32;
    let file_eid = file_t
        .insert()
        .set("path", path_s)
        .set("crate_", crate_name.clone())
        .set("lang", LANG_RUST)
        .set("loc", loc)
        .set("hash", hash_u32(src))
        .commit()
        .unwrap();
    let rel_path = rel_of(path_s, root);
    acc.file_by_rel.insert(rel_path.clone(), file_eid);
    let mut ctx = Ctx {
        file_eid,
        rel_path,
        crate_name,
        module: Vec::new(),
        in_test: false,
        container: String::new(),
        n_sym: 0,
    };
    walk_items(&file.items, &mut ctx, acc, sym_t);
    true
}

/// walkdir で dir 以下の index 対象 .rs を列挙 (target/ と隠し dir は除外)。
fn rust_files(dir: &str) -> impl Iterator<Item = std::path::PathBuf> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        // ビルド生成物 (target) と隠しディレクトリ全部 (.git / .claude / .turbo …) を
        // **ディレクトリごと枝刈り** — rg の hidden 除外と同じ規約。正規の Rust source は隠し dir に
        // 住まない一方、VCS/ツールの内部 store が .rs を持ち込むと重複混入する。
        // (パス文字列の後段フィルタだと target/ 数十万ファイルを列挙してから捨てる = staleness
        //  stat-walk が毎クエリ秒単位になる。filter_entry なら降りない。)
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true; // root 自身は名前に依らず歩く ("." 起動や隠し dir 直指定を殺さない)
            }
            let n = e.file_name().to_string_lossy();
            n != "target" && !n.starts_with('.')
        })
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let p = entry.path();
            if p.extension().is_none_or(|e| e != "rs") {
                return None;
            }
            Some(p.to_path_buf())
        })
}

// ─────────────────────────── index コマンド ───────────────────────────

/// index のエントリ。table の eid 予約はファイル数から見積もるが、symbol 密度が高い
/// プロジェクト (コンパイラ等) では過小になり枯渇し得る。その時は capacity を上げて自動リトライ
/// (tight-by-default で open を速く保ちつつ、外れ値でも落ちない)。
pub fn run_index(dir: &str, path: &str, scip_path: Option<&str>) {
    let mut cap_mult = 1u32;
    loop {
        // 予約枯渇 (enchudb の unwrap 失敗) を捕まえるため panic を握りつぶして試行。
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_index_inner(dir, path, scip_path, cap_mult)
        }));
        std::panic::set_hook(prev);
        // panic payload を人間可読に (握りつぶすと真因が消えるので必ず表示する)。
        let msg_of = |e: Box<dyn std::any::Any + Send>| -> String {
            e.downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "<non-string panic>".into())
        };
        match r {
            Ok(()) => return,
            Err(e) if cap_mult < 64 => {
                cap_mult *= 4;
                eprintln!("# index 失敗 ({}) → capacity {cap_mult}x で再試行", msg_of(e));
            }
            Err(e) => {
                eprintln!("# index 失敗: capacity 64x でも解消せず ({dir}): {}", msg_of(e));
                return;
            }
        }
    }
}

fn run_index_inner(dir: &str, path: &str, scip_path: Option<&str>, cap_mult: u32) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}.oplog", path));
    let _ = std::fs::remove_file(format!("{}.lock", path));

    eprintln!("=== kenning index: {} → {} ===", dir, path);
    if let Some(sp) = scip_path {
        eprintln!("(SCIP 正確解決: {})", sp);
    }

    // eid 予約は実データ規模 (ファイル数) から見積もる。過剰予約はファイルを太らせ open を遅くする。
    // growable なので不足したら enchudb が伸ばす (build 時コストのみ)。ref は --scip 時のみ大きく。
    // cap_mult は枯渇リトライ時に上がる (1 → 4 → 16 → 64)。file 数は不変なので掛けない。
    let n_files_est = rust_files(dir).count().max(1) as u32;
    let file_cap = (n_files_est * 2).max(1_024);
    let sym_cap = (n_files_est * 64).max(8_192) * cap_mult; // enchudb 実測 ~17 sym/file、余裕 64
    let call_cap = (n_files_est * 400).max(32_768) * cap_mult; // 実測 ~154 call/file、余裕 400
    let ref_cap = if scip_path.is_some() { (n_files_est * 400).max(32_768) * cap_mult } else { 1_024 };
    let impl_cap = (n_files_est * 16).max(1_024) * cap_mult; // impl Trait for Type edge
    let extref_cap = if scip_path.is_some() { (n_files_est * 100).max(8_192) * cap_mult } else { 1_024 };
    let max_entities = (file_cap + sym_cap + call_cap + ref_cap + impl_cap + extref_cap) * 11 / 10; // +10% 余白
    let mut db = Database::create_growable_with_capacity(path, max_entities).unwrap();
    db.table("file")
        .tag("path")
        .tag("crate_")
        .number("lang")
        .number("loc")
        .number("hash") // 内容 fingerprint (増分 index の変更検知)
        .with_capacity(file_cap)
        .build()
        .unwrap();
    db.table("sym")
        .tag("name")
        .number("kind")
        .number("vis")
        .number("is_async")
        .number("is_test")
        .ref_to("file", "file")
        .tag("module")
        .tag("crate_")
        .tag("container")
        .tag("symbol") // SCIP グローバル一意 symbol (無指定なら "")。cross-file 同一性の鍵
        .tag("sig") // fn/method のシグネチャ 1 行 (hover 相当。他 kind は "")
        .tag("doc") // doc コメント 1 行目 (無ければ ""。def/outline で sig と並べて出す)
        .number("line")
        .number("end_line") // item 終端行 (`read` の切り出し下端。旧 index は 0 = 開始行のみ)
        .with_capacity(sym_cap)
        .build()
        .unwrap();
    db.table("call")
        .ref_to("caller", "sym")
        .tag("callee")            // 単純名 (text fallback / 表示用)
        .ref_to("callee_sym", "sym") // 解決先 sym (未解決なら未 set = Null)
        .number("res")            // 信頼度 (R_UNRESOLVED..R_AMBIG)
        .tag("qual")              // 修飾 (Type::/mod::。無ければ "")。増分の再解決用に永続化
        .number("is_method")      // x.f() 形式か。同上
        .ref_to("file", "file")
        .number("line")
        .with_capacity(call_cap)
        .build()
        .unwrap();
    // 全参照 edge (SCIP occurrence)。who-calls の上位互換 = find-all-refs。
    db.table("ref")
        .ref_to("symbol_sym", "sym") // 参照先の workspace sym
        .ref_to("file", "file")
        .number("line")
        .number("col")
        .number("role") // SCIP symbol_roles bit (1=def 2=import 4=write 8=read)
        .with_capacity(ref_cap)
        .build()
        .unwrap();
    // 外部 crate への参照 (SCIP occurrence のうち workspace 外・std 以外)。cross-repo refs の鍵:
    // SCIP symbol はグローバル一意 (crate+version) なので、別 repo の db の sym.symbol と突き合わせると
    // 「この repo が enchudb::X をどこで使うか」が repo を跨いで精密に出る (`across`)。
    db.table("extref")
        .tag("symbol") // 外部シンボルのグローバル一意文字列
        .ref_to("file", "file")
        .number("line")
        .number("col")
        .number("role")
        .with_capacity(extref_cap)
        .build()
        .unwrap();
    // impl Trait for Type edge (go-to-implementation)。file 別 = 増分 update で purge/再挿入。
    db.table("impl")
        .tag("trait_name")
        .tag("type_name")
        .ref_to("file", "file")
        .number("line")
        .with_capacity(impl_cap)
        .build()
        .unwrap();
    // 自己記述メタ (1 行): どの root をいつ index したか。staleness 警告と `update <db>` に使う。
    db.table("meta")
        .tag("root") // index した絶対 dir
        .number("built_at") // index 時刻 (unix 秒)
        .number("nfiles")
        .number("ver") // INDEX_VER (意味論の版。違えば増分せず full 再 index)
        .number("baked_at") // SCIP bake 時刻 (0 = 未 bake)。freshness 2層の SCIP 側
        .number("bake_peak_mb") // 前回 bake の peak RSS (次回ゲートの見積り)
        .number("upd_since_bake") // bake 後に増分 update したファイル数 (閾値で bake 推奨)
        .with_capacity(16)
        .build()
        .unwrap();

    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let ref_t = db.get_table("ref").unwrap();

    let mut n_files = 0u64;
    let mut n_skip = 0u64;
    let mut acc = Acc::default();
    // SCIP があれば pass1 の前にロード (sym の symbol タグ付けに使う)。
    if let Some(sp) = scip_path {
        acc.scip = Some(Scip::load(sp, dir));
    }
    let t = Instant::now();

    // ── pass1: 全ファイルを歩いて sym を挿入 + call-site を deferred 収集 ──
    for p in rust_files(dir) {
        let ps = p.to_string_lossy();
        let Ok(src) = std::fs::read_to_string(&p) else { continue };
        if index_one_file(&file_t, &sym_t, &mut acc, dir, &ps, &src) {
            n_files += 1;
        } else {
            n_skip += 1;
        }
    }
    let n_sym = sym_t.all().count().unwrap() as u64;
    let parse_el = t.elapsed();

    // ── pass2: callee を解決して call を挿入。SCIP があれば位置 join、無ければ syn ヒューリ ──
    let t2 = Instant::now();
    let n_call = acc.pending.len() as u64;
    let mut res_counts = [0u64; 4]; // [unresolved, unique, qualified, ambiguous]
    let mut scip_ws = 0u64; // SCIP occurrence が workspace 定義に確定
    let mut scip_external = 0u64; // SCIP は symbol を知ってるが自 index 外 (std/dep)
    let mut syn_recovered = 0u64; // SCIP 沈黙 (no-occ) を syn ヒューリで拾った数
    let diag = std::env::var_os("KENNING_DIAG_NOOCC").is_some();
    let mut d_nodoc = 0u64; // no-occ かつ SCIP に doc 自体が無い (test-helper/例外ファイル)
    let mut d_indoc = 0u64; // no-occ だが doc はある = 位置ズレ (macro/closure 等)
    let mut d_indoc_method = 0u64; // うち method 呼び (x.f())
    let mut d_samples: Vec<String> = Vec::new();
    for cs in &acc.pending {
        let (target, res) = if let Some(scip) = &acc.scip {
            match scip.symbol_at(&cs.rel_path, cs.line, cs.col) {
                Some(sym) => match acc.sym_by_symbol.get(sym) {
                    Some(&eid) => {
                        scip_ws += 1;
                        (Some(eid), R_UNIQUE) // workspace の定義に確定
                    }
                    None => {
                        scip_external += 1;
                        (None, R_UNRESOLVED) // std/dep/macro (SCIP は知ってるが自 index に無い)
                    }
                },
                // SCIP がこの位置に occurrence を出さない = 主に cfg 非活性コード
                // (syn は cfg-blind に全ブランチを parse するが RA は活性 cfg のみ解析) や
                // macro 生成領域。SCIP は沈黙なので syn ヒューリで best-effort 解決を試みる。
                None => {
                    if diag {
                        if scip.has_doc(&cs.rel_path) {
                            d_indoc += 1;
                            if cs.is_method {
                                d_indoc_method += 1;
                            }
                            if d_samples.len() < 25 {
                                d_samples.push(format!(
                                    "  {}:{}:{} {}{}{}",
                                    cs.rel_path, cs.line, cs.col,
                                    if cs.is_method { "." } else { "" },
                                    cs.name,
                                    cs.qualifier.as_ref().map(|q| format!(" (qual={})", q)).unwrap_or_default(),
                                ));
                            }
                        } else {
                            d_nodoc += 1;
                        }
                    }
                    let (t, r) = resolve_call(cs, &acc.defs);
                    if t.is_some() {
                        syn_recovered += 1;
                    }
                    (t, r)
                }
            }
        } else {
            resolve_call(cs, &acc.defs)
        };
        res_counts[res as usize] += 1;
        do_insert_call(&call_t, cs, target, res);
    }
    if diag {
        eprintln!(
            "[DIAG] no-occ 内訳: doc欠落={} / doc有り(位置ズレ)={} (うち method={}, path/fn={}) | syn 回収={}",
            d_nodoc, d_indoc, d_indoc_method, d_indoc - d_indoc_method, syn_recovered
        );
        eprintln!("[DIAG] doc有り no-occ サンプル:");
        for s in &d_samples {
            eprintln!("{}", s);
        }
    }
    let resolve_el = t2.elapsed();
    let resolved = res_counts[R_UNIQUE as usize] + res_counts[R_QUALIFIED as usize];

    eprintln!(
        "indexed: {} files / {} symbols / {} call-sites (parse {:?} + resolve {:?}, {} parse-skip)",
        n_files, n_sym, n_call, parse_el, resolve_el, n_skip
    );
    if acc.scip.is_some() {
        eprintln!(
            "resolve[SCIP]: {} / {} 解決 ({:.1}%) = SCIP確定 {} + syn回収 {} (cfg非活性/SCIP沈黙を best-effort) — external/std {} (SCIP識別), 未解決 {}",
            resolved,
            n_call,
            if n_call > 0 { resolved as f64 * 100.0 / n_call as f64 } else { 0.0 },
            scip_ws,
            syn_recovered,
            scip_external,
            res_counts[R_UNRESOLVED as usize] - scip_external,
        );
    } else {
        eprintln!(
            "resolve[syn]: {} / {} call-sites 解決 ({:.1}%) — unique {} / qualified {} / ambiguous {} / external {}",
            resolved,
            n_call,
            if n_call > 0 { resolved as f64 * 100.0 / n_call as f64 } else { 0.0 },
            res_counts[R_UNIQUE as usize],
            res_counts[R_QUALIFIED as usize],
            res_counts[R_AMBIG as usize],
            res_counts[R_UNRESOLVED as usize],
        );
    }

    // ── ref-ingest: SCIP の全 occurrence を workspace sym への参照 edge にする (find-all-refs) ──
    if let Some(scip) = &acc.scip {
        let t3 = Instant::now();
        let extref_t = db.get_table("extref").unwrap();
        let mut n_ref = 0u64;
        let mut n_ext = 0u64;
        for o in &scip.occ {
            let Some(&file_eid) = acc.file_by_rel.get(&o.rel_path) else { continue };
            match acc.sym_by_symbol.get(&o.symbol) {
                Some(&sym_eid) => {
                    ref_t
                        .insert()
                        .set("symbol_sym", Value::Ref(sym_eid))
                        .set("file", Value::Ref(file_eid))
                        .set("line", o.line0 + 1) // 1-indexed に揃える
                        .set("col", o.col0)
                        .set("role", o.roles as u32)
                        .commit()
                        .unwrap();
                    n_ref += 1;
                }
                None => {
                    // 外部シンボル: std/toolchain と local は捨て、dep crate への参照だけ残す
                    // (cross-repo `across` の材料。std を入れると数十万行のノイズになる)。
                    if o.symbol.contains("https://github.com/rust-lang/rust/library") || o.symbol.starts_with("local ") {
                        continue;
                    }
                    extref_t
                        .insert()
                        .set("symbol", o.symbol.as_str())
                        .set("file", Value::Ref(file_eid))
                        .set("line", o.line0 + 1)
                        .set("col", o.col0)
                        .set("role", o.roles as u32)
                        .commit()
                        .unwrap();
                    n_ext += 1;
                }
            }
        }
        drop(extref_t);
        eprintln!("ref: {} workspace 参照 + {} 外部 crate 参照 (extref) を edge 化 ({:?})", n_ref, n_ext, t3.elapsed());
    }

    // ── impl edge を焼く (impl Trait for Type)。go-to-implementation 用。 ──
    let impl_t = db.get_table("impl").unwrap();
    for ie in &acc.impls {
        impl_t
            .insert()
            .set("trait_name", ie.trait_name.as_str())
            .set("type_name", ie.type_name.as_str())
            .set("file", Value::Ref(ie.file))
            .set("line", ie.line)
            .commit()
            .unwrap();
    }
    eprintln!("impl: {} 個の impl Trait for Type edge", acc.impls.len());
    drop(impl_t);

    // 自己記述メタを焼く (root は絶対パスに正規化して、cwd に依らず update/staleness を効かせる)。
    let meta_t = db.get_table("meta").unwrap();
    let root_abs = std::fs::canonicalize(dir).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| dir.to_string());
    let mut ins = meta_t
        .insert()
        .set("root", root_abs.as_str())
        .set("built_at", now_secs())
        .set("nfiles", n_files as u32)
        .set("ver", INDEX_VER);
    // SCIP を食ったならこの index は baked。時刻は .scip の mtime (= RA が facts を生成した時) —
    // heal/移行の再 index で bake スタンプが消えないように (bake コマンド自身は後で now に上書き)。
    if let Some(sp) = scip_path {
        if let Some(t) = std::fs::metadata(sp)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        {
            ins = ins.set("baked_at", t.as_secs() as u32).set("upd_since_bake", 0u32);
        }
    }
    ins.commit().unwrap();

    drop(file_t);
    drop(sym_t);
    drop(call_t);
    drop(ref_t);
    drop(meta_t);
    let db = db.finish_with_oplog(OPLOG_CAPACITY).unwrap();
    drop(db);

    let real = std::fs::metadata(path)
        .map(|m| {
            use std::os::unix::fs::MetadataExt;
            (m.blocks() * 512) as f64 / 1_048_576.0
        })
        .unwrap_or(0.0);
    eprintln!("db real disk: {:.1} MB", real);
    eprintln!("\n次: `kenning def <name>` / `callers <name>` / `search kind:fn vis:pub`");
}

// ─────────────────────────── update (増分 index) ───────────────────────────

/// 既存 index を開き、内容 hash が変わった / 追加 / 削除されたファイルだけ再 index する。
/// 未変更ファイルは再パースしない (増分の肝)。名前解決の一貫性は、変更で影響を受けた
/// symbol 名 (`affected`) の incoming call を再解決することで保つ ⇒ full 再 index と同一結果。
pub fn run_update(dir: &str, path: &str) {
    eprintln!("=== kenning update: {} → {} ===", dir, path);
    // 既存 DB を書込可能で開く (drop で永続 / entity_in は新 eid を再発行)。
    // db ファイルがまだ無ければ full index にフォールバック (update = 初回でも動く)。
    // ※ ファイルが在るのに open 失敗 (= lock 等) は full にしない — 消して作り直す事故を防ぐ。
    let db = match Database::open(path) {
        Ok(db) => db,
        Err(e) => {
            if std::path::Path::new(path).exists() {
                eprintln!("# index を開けない ({e})。他プロセス使用中かも → 後で再試行を。");
            } else {
                eprintln!("(index が無いので full index します)");
                run_index(dir, path, None);
            }
            return;
        }
    };
    update_with_heal(db, dir, path);
}

/// update を試み、失敗 (旧 schema の index 等で panic) したら full 再 index で自己修復。
/// bake 済みの .scip が残っていれば精度も維持して焼き直す。
fn update_with_heal(db: Database, dir: &str, path: &str) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| update_inner(db, dir)));
    std::panic::set_hook(prev);
    if r.is_err() {
        let scip = format!("{}.scip", path.trim_end_matches(".db"));
        let scip = if std::path::Path::new(&scip).exists() { Some(scip) } else { None };
        eprintln!(
            "# 増分 update 失敗 (旧 schema の index?) → full 再 index で自己修復{}",
            if scip.is_some() { " (.scip 再利用で精度維持)" } else { "" }
        );
        run_index(dir, path, scip.as_deref());
    }
}

/// update の本体 (open 済み db を受け取る)。auto-update (maybe_auto_update) と run_update が共用。
fn update_inner(db: Database, dir: &str) {
    // index 意味論の版が違えば増分は不整合 (旧値と新値が混ざる) → panic して
    // update_with_heal の full 再 index (.scip 再利用) に落とす。
    if let Some(meta_t) = db.get_table("meta") {
        if let Some(e) = meta_t.all().find().unwrap().into_iter().next() {
            let v = match meta_t.entity(e).get("ver") {
                Some(Value::Number(n)) => n as u32,
                _ => 0,
            };
            if v != INDEX_VER {
                panic!("index ver {v} != {INDEX_VER} (意味論変更) → full 再 index が必要");
            }
        }
    }
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let impl_t = db.get_table("impl"); // 旧 index には無い (Option)
    let t = Instant::now();

    // 1. 既存 file 表 (path → (eid, hash))。
    let mut prev: HashMap<String, (EntityId, u32)> = HashMap::new();
    for fe in file_t.all().find().unwrap() {
        let er = file_t.entity(fe);
        prev.insert(txt(er.get("path")), (fe, num(er.get("hash"))));
    }

    // 2. 現在の disk を走査 (path → src)。
    let mut cur: HashMap<String, String> = HashMap::new();
    for p in rust_files(dir) {
        if let Ok(src) = std::fs::read_to_string(&p) {
            cur.insert(p.to_string_lossy().into_owned(), src);
        }
    }

    // 3. 分類。to_add = 変更 ∪ 新規 (再 index)、to_remove = 変更 ∪ 削除 (旧 facts 消去)。
    let mut to_add: Vec<(String, String)> = Vec::new();
    let mut to_remove: Vec<EntityId> = Vec::new();
    for (p, src) in &cur {
        match prev.get(p) {
            Some((eid, h)) => {
                if *h != hash_u32(src) {
                    to_remove.push(*eid);
                    to_add.push((p.clone(), src.clone()));
                }
            }
            None => to_add.push((p.clone(), src.clone())),
        }
    }
    let mut n_deleted = 0u64;
    for (p, (eid, _)) in &prev {
        if !cur.contains_key(p) {
            to_remove.push(*eid);
            n_deleted += 1;
        }
    }

    if to_add.is_empty() && to_remove.is_empty() {
        eprintln!("変更なし ({} files 走査、{:?})。", cur.len(), t.elapsed());
        // built_at だけ再スタンプして返る。これをしないと mtime だけ変わったファイル (touch 等) が
        // 毎クエリ「古い」判定 → 空 update が永遠に走り続ける (実測で踏んだバグ)。
        if let Some(meta_t) = db.get_table("meta") {
            if let Some(e) = meta_t.all().find().unwrap().into_iter().next() {
                let er = meta_t.entity(e);
                let (root_s, nfiles) = (txt(er.get("root")), num(er.get("nfiles")));
                let (baked, peak, upd) = (er.get("baked_at"), er.get("bake_peak_mb"), er.get("upd_since_bake"));
                meta_t.entity(e).delete().unwrap();
                let mut ins = meta_t.insert().set("root", root_s.as_str()).set("built_at", now_secs()).set("nfiles", nfiles).set("ver", INDEX_VER);
                if let Some(Value::Number(b)) = baked {
                    ins = ins.set("baked_at", b as u32);
                    if let Some(Value::Number(p)) = peak { ins = ins.set("bake_peak_mb", p as u32); }
                    if let Some(Value::Number(u)) = upd { ins = ins.set("upd_since_bake", u as u32); }
                }
                ins.commit().unwrap();
            }
        }
        return;
    }

    // 4. 変更/削除ファイルの旧 facts を消去 (影響 symbol 名を集める)。
    let mut affected: HashSet<String> = HashSet::new();
    for eid in &to_remove {
        purge_file(&sym_t, &call_t, &file_t, impl_t.as_ref(), *eid, &mut affected);
    }

    // 5. 追加/変更ファイルを parse して新 sym を挿入 + call-site を持ち越す。
    let mut acc = Acc::default();
    let mut n_skip = 0u64;
    for (p, src) in &to_add {
        if !index_one_file(&file_t, &sym_t, &mut acc, dir, p, src) {
            n_skip += 1;
        }
    }
    for name in acc.defs.keys() {
        affected.insert(name.clone()); // 新規定義した名前も incoming 再解決の対象
    }

    // 6. 全 sym から global defs を再構築 (新旧すべて反映、再パース不要)。
    let defs = build_defs_from_table(&sym_t);

    // 7. incoming 再解決: 影響名を callee に持つ「既存 (=未変更ファイル) の call」を
    //    delete + 再挿入して callee_sym/res を最新化。この時点で存在する該当 call は
    //    未変更ファイル由来のみ (変更/削除は 4 で purge 済、新規は 8 で未挿入)。
    let mut reresolved = 0u64;
    for name in &affected {
        for ce in call_t.where_eq("callee", name.as_str()).find().unwrap() {
            let er = call_t.entity(ce);
            let Some(Value::Ref(caller)) = er.get("caller") else { continue };
            let Some(Value::Ref(file)) = er.get("file") else { continue };
            let qual_s = txt(er.get("qual"));
            let cs = CallSite {
                caller,
                caller_container: txt(sym_t.entity(caller).get("container")),
                file,
                rel_path: String::new(), // update は syn 再解決 (rel_path/col は使わない)
                name: name.clone(),
                qualifier: if qual_s.is_empty() { None } else { Some(qual_s) },
                is_method: num(er.get("is_method")) == 1,
                line: num(er.get("line")),
                col: 0,
            };
            call_t.entity(ce).delete().unwrap();
            insert_call(&call_t, &cs, &defs);
            reresolved += 1;
        }
    }

    // 8. 追加/変更ファイルの outgoing call を解決して挿入。
    let mut n_new_call = 0u64;
    for cs in &acc.pending {
        insert_call(&call_t, cs, &defs);
        n_new_call += 1;
    }

    // 8b. 追加/変更ファイルの impl edge を挿入 (旧 facts は 4 で purge 済)。
    if let Some(it) = &impl_t {
        for ie in &acc.impls {
            it.insert()
                .set("trait_name", ie.trait_name.as_str())
                .set("type_name", ie.type_name.as_str())
                .set("file", Value::Ref(ie.file))
                .set("line", ie.line)
                .commit()
                .unwrap();
        }
    }

    let n_changed = to_add.len() as u64;
    eprintln!(
        "update: {} 再 index / {} 削除 / {} 未変更 ({:?}, {} parse-skip)",
        n_changed,
        n_deleted,
        cur.len() as u64 - n_changed,
        t.elapsed(),
        n_skip,
    );
    eprintln!(
        "  新規 outgoing call {} / incoming 再解決 {} / affected 名前 {}",
        n_new_call,
        reresolved,
        affected.len(),
    );

    // meta を再スタンプ (built_at を現在に)。bake 情報は持ち越し + 変更数を積算 (閾値で bake 推奨)。
    // 旧 index (meta 表 / bake 列なし) は present なフィールドだけ扱い後方互換。
    if let Some(meta_t) = db.get_table("meta") {
        let root_abs = std::fs::canonicalize(dir).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| dir.to_string());
        let nfiles = file_t.all().count().unwrap() as u32;
        let old = meta_t.all().find().unwrap().into_iter().next();
        let (baked_at, peak_mb, upd_sb) = old
            .map(|e| {
                let er = meta_t.entity(e);
                (er.get("baked_at"), er.get("bake_peak_mb"), er.get("upd_since_bake"))
            })
            .unwrap_or((None, None, None));
        for e in meta_t.all().find().unwrap() {
            meta_t.entity(e).delete().unwrap(); // 更新は delete+reinsert (Null tie を避ける)
        }
        let mut ins = meta_t.insert().set("root", root_abs.as_str()).set("built_at", now_secs()).set("nfiles", nfiles).set("ver", INDEX_VER);
        if let Some(Value::Number(b)) = baked_at {
            let upd = match upd_sb { Some(Value::Number(u)) => u as u32, _ => 0 } + n_changed as u32;
            ins = ins.set("baked_at", b as u32).set("upd_since_bake", upd);
            if let Some(Value::Number(p)) = peak_mb {
                ins = ins.set("bake_peak_mb", p as u32);
            }
            if b > 0 && upd >= 20 {
                eprintln!("# SCIP facts が古くなってきた (bake 後 {upd} ファイル変更) → `kenning bake` 推奨");
            }
        }
        ins.commit().unwrap();
        drop(meta_t);
    }

    drop(file_t);
    drop(sym_t);
    drop(call_t);
    drop(impl_t);
    drop(db); // standalone: drop で schema + data を永続化。
    eprintln!("\n次: `kenning def <name>` / `callers <name>` / `search kind:fn vis:pub`");
}

/// `update <db>` — dir 省略時: meta の root を読んで再 index (自己記述 index の活用)。
pub fn run_update_from_db(path: &str) {
    let root = match Database::open_readonly(path) {
        Ok(db) => read_meta(&db).map(|(r, _)| r),
        Err(e) => {
            eprintln!("# index を開けない ({path}): {e}");
            eprintln!("# 先に: kenning index <dir> {path}");
            return;
        }
    };
    match root {
        Some(r) if !r.is_empty() => run_update(&r, path),
        _ => eprintln!("# この index に root 情報が無い (旧 index)。`kenning update <dir> {path}` で dir 指定を。"),
    }
}

// ─────────────────────────── bake (RA = オフラインの焼き窯) ───────────────────────────
//
// `kenning bake [dir]` = rust-analyzer scip を回して --scip 再 index まで一発。
// RA は常駐させず、精度が要る時だけバッチで焚く (peak ~5GB × 数十秒、常駐 0)。
// ガード: (i) 空きメモリゲート (足りなければ焚かない) (ii) グローバル lock で直列化
// (iii) features は --config-path で all を注入 (GIGO 対策、Cargo.toml は触らない)。

/// bake 直列化 lock。Drop で必ず解放 (パニック時も unwind で外れる)。
/// SIGTERM/SIGKILL では Drop が走らず残骸化するので、衝突時は lock 内 pid の生存を
/// 確認して、死んでいれば自動回収する (issue #1)。best-effort 直列化なので TOCTOU は許容。
struct BakeLock(std::path::PathBuf);
impl BakeLock {
    fn acquire(cache: &std::path::Path) -> Option<BakeLock> {
        Self::try_acquire(cache, true)
    }
    fn try_acquire(cache: &std::path::Path, reclaim: bool) -> Option<BakeLock> {
        let p = cache.join("bake.lock");
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(mut f) => {
                use std::io::Write;
                let _ = write!(f, "{}", std::process::id());
                Some(BakeLock(p))
            }
            Err(_) => {
                let pid = std::fs::read_to_string(&p).unwrap_or_default().trim().to_string();
                if reclaim && !pid_alive(&pid) {
                    eprintln!("# stale な bake.lock (pid {pid} は死亡) を自動回収");
                    let _ = std::fs::remove_file(&p);
                    return Self::try_acquire(cache, false); // 回収→再取得は 1 回だけ (race したら諦める)
                }
                eprintln!("# 別の bake が進行中 (pid {pid})。バースト積層防止のため直列化してる。");
                eprintln!("# 異常終了の残骸なら: rm {}", p.display());
                None
            }
        }
    }
}

/// pid の生存確認 (`kill -0`)。空/非数値も「死亡」扱い — 正常な lock は必ず自 pid を
/// 書くので、読めない = 書き込み途中で死んだ残骸。kill が引けない環境でも安全側 (回収) に倒す。
fn pid_alive(pid: &str) -> bool {
    if pid.parse::<u32>().is_err() {
        return false;
    }
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
impl Drop for BakeLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 空きメモリ (MB)。macOS: memory_pressure の free% × hw.memsize。測れなければ None (ゲートは警告のみ)。
fn avail_mem_mb() -> Option<u64> {
    let pct = std::process::Command::new("memory_pressure").arg("-Q").output().ok()?;
    let s = String::from_utf8_lossy(&pct.stdout);
    let pct: u64 = s.lines().find(|l| l.contains("free percentage"))?.trim_end_matches('%').rsplit(' ').next()?.parse().ok()?;
    let total = std::process::Command::new("sysctl").args(["-n", "hw.memsize"]).output().ok()?;
    let total: u64 = String::from_utf8_lossy(&total.stdout).trim().parse().ok()?;
    Some(total / 1_048_576 * pct / 100)
}

/// rust-analyzer binary を探す: env KENNING_RA > PATH > rustup toolchain 直。
fn find_ra() -> Option<String> {
    let mut cands: Vec<String> = Vec::new();
    if let Ok(p) = std::env::var("KENNING_RA") {
        cands.push(p);
    }
    cands.push("rust-analyzer".to_string());
    if let Ok(home) = std::env::var("HOME") {
        if let Ok(rd) = std::fs::read_dir(format!("{home}/.rustup/toolchains")) {
            for e in rd.flatten() {
                cands.push(e.path().join("bin/rust-analyzer").to_string_lossy().to_string());
            }
        }
    }
    cands.into_iter().find(|c| {
        std::process::Command::new(c)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

pub fn run_bake(dir: &str) {
    let Some(root) = repo_root_of(dir) else {
        eprintln!("# repo root が見つからない ({dir})。repo 内で実行を。");
        std::process::exit(2);
    };
    let root_s = root.to_string_lossy().to_string();
    let Some(db) = default_db_for(&root_s) else {
        eprintln!("# db パスを導出できない。");
        std::process::exit(2);
    };
    let cache = std::path::Path::new(&db).parent().map(|p| p.to_path_buf()).unwrap_or_else(std::env::temp_dir);

    // ── ゲート: 空きメモリ。必要量 = 前回 peak × 1.3 (無ければ保守的に 6GB)。 ──
    let needed_mb = Database::open_readonly(&db)
        .ok()
        .and_then(|d| {
            let mt = d.get_table("meta")?;
            let e = mt.all().find().ok()?.into_iter().next()?;
            match mt.entity(e).get("bake_peak_mb") {
                Some(Value::Number(p)) if p > 0 => Some(p as u64 * 13 / 10),
                _ => None,
            }
        })
        .unwrap_or(6144);
    match avail_mem_mb() {
        Some(avail) if avail < needed_mb && std::env::var_os("KENNING_BAKE_FORCE").is_none() => {
            eprintln!("# 空きメモリ不足: 空き {avail}MB < 必要見込み {needed_mb}MB → 焚かない。");
            eprintln!("# 空けてから再実行 or KENNING_BAKE_FORCE=1 (自己責任) or 強いマシンで焼いた .scip を `index --scip` で。");
            std::process::exit(3);
        }
        Some(avail) => eprintln!("# gate ok: 空き {avail}MB ≧ 必要見込み {needed_mb}MB"),
        None => eprintln!("# ⚠ 空きメモリを測れない → ゲートなしで続行"),
    }

    // ── 直列化 lock ──
    let Some(_lock) = BakeLock::acquire(&cache) else { std::process::exit(3) };

    let Some(ra) = find_ra() else {
        eprintln!("# rust-analyzer が見つからない。`rustup component add rust-analyzer` か env KENNING_RA で指定を。");
        std::process::exit(2);
    };

    // ── SCIP 生成 (features=all を --config-path で注入。Cargo.toml は触らない) ──
    let scip_path = format!("{}.scip", db.trim_end_matches(".db"));
    let cfg_path = format!("{}.racfg.json", db.trim_end_matches(".db"));
    let use_all = std::env::var_os("KENNING_BAKE_DEFAULT_FEATURES").is_none();
    let n_src = rust_files(&root_s).count();
    let mut peak_mb = 0u64;
    let mut baked = false;
    for attempt in 0..2 {
        let all = use_all && attempt == 0;
        if all {
            std::fs::write(&cfg_path, r#"{"cargo": {"features": "all"}}"#).unwrap();
        }
        eprintln!(
            "# bake: rust-analyzer scip {} (features={}) — peak ~{:.1}GB / 数十秒〜数分、常駐なし",
            root_s, if all { "all" } else { "default" }, needed_mb as f64 / 1024.0
        );
        let mut cmd = std::process::Command::new("/usr/bin/time");
        cmd.args(["-l", &ra, "scip", &root_s, "--output", &scip_path]);
        if all {
            cmd.args(["--config-path", &cfg_path]);
        }
        let out = cmd.current_dir(&root_s).output().expect("spawn time+rust-analyzer");
        let errs = String::from_utf8_lossy(&out.stderr);
        peak_mb = errs
            .lines()
            .find(|l| l.contains("maximum resident set size"))
            .and_then(|l| l.trim().split(' ').next())
            .and_then(|n| n.parse::<u64>().ok())
            .map(|b| b / 1_048_576)
            .unwrap_or(0);
        if !out.status.success() || !std::path::Path::new(&scip_path).exists() {
            eprintln!("# bake 失敗 (features={}):", if all { "all" } else { "default" });
            eprintln!("{}", errs.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
            if all { continue; } else { std::process::exit(1); }
        }
        // sanity: features=all が相互排他 cfg 等で薄い SCIP を吐いたら default で焼き直す。
        let n_docs = std::fs::read(&scip_path)
            .ok()
            .and_then(|b| {
                use protobuf::Message;
                scip::types::Index::parse_from_bytes(&b).ok()
            })
            .map(|i| i.documents.len())
            .unwrap_or(0);
        if all && n_docs * 2 < n_src {
            eprintln!("# ⚠ features=all の SCIP が薄い (doc {n_docs} / src {n_src}) → default features で焼き直し");
            continue;
        }
        eprintln!("# bake 完了: {} docs / peak {}MB", n_docs, peak_mb);
        baked = true;
        break;
    }
    if !baked {
        eprintln!("# bake 失敗 (all/default 両方)");
        std::process::exit(1);
    }
    let _ = std::fs::remove_file(&cfg_path);

    // ── SCIP 込みで full 再 index → meta に bake 情報を焼く ──
    run_index(&root_s, &db, Some(&scip_path));
    if let Ok(dbw) = Database::open(&db) {
        if let Some(mt) = dbw.get_table("meta") {
            let old = mt.all().find().unwrap().into_iter().next().map(|e| {
                let er = mt.entity(e);
                (txt(er.get("root")), num(er.get("built_at")), num(er.get("nfiles")))
            });
            for e in mt.all().find().unwrap() {
                mt.entity(e).delete().unwrap();
            }
            if let Some((r, b, n)) = old {
                mt.insert()
                    .set("root", r.as_str())
                    .set("built_at", b)
                    .set("nfiles", n)
                    .set("ver", INDEX_VER)
                    .set("baked_at", now_secs())
                    .set("bake_peak_mb", peak_mb as u32)
                    .set("upd_since_bake", 0u32)
                    .commit()
                    .unwrap();
            }
        }
    }
    eprintln!("# 精密 facts 有効: refs / callers が RA 同等精度に (`kenning refs <name>`)");
}

// ═══════════════════════ 探索コマンド (Claude 向け、token 効率重視) ═══════════════════════
//
// 出力は `path:line<TAB>詳細` の 1 行 = そのまま Read に渡せる。装飾/計測なし、決定的順序。
// 目的は「grep 全マッチ + ファイル通読」を「精密な少数行」に圧縮すること。

/// 現在時刻 (unix 秒)。index 時刻の記録に使う。
fn now_secs() -> u32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0)
}

/// 自己記述メタ (root, built_at) を読む。旧 index (meta 無し) なら None。
fn read_meta(db: &Database) -> Option<(String, u32)> {
    let meta_t = db.get_table("meta")?;
    let e = meta_t.all().find().ok()?.into_iter().next()?;
    let er = meta_t.entity(e);
    Some((txt(er.get("root")), num(er.get("built_at"))))
}

/// root 以下で index 時刻より新しい .rs の数 (stat のみ、read しないので軽い)。
fn count_stale(root: &str, built_at: u32) -> usize {
    rust_files(root)
        .filter(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .is_some_and(|d| d.as_secs() as u32 > built_at)
        })
        .count()
}

/// index が古ければ stderr に警告 (stdout の path:line は汚さない)。KENNING_NO_STALE で無効化。
fn warn_if_stale(db: &Database, db_path: &str) {
    if std::env::var_os("KENNING_NO_STALE").is_some() || STALE_CHECKED.load(Ordering::Relaxed) {
        return; // auto 経路が同一プロセスで確認/更新済みなら walk を繰り返さない
    }
    let Some((root, built_at)) = read_meta(db) else { return };
    if root.is_empty() || built_at == 0 {
        return;
    }
    let n = count_stale(&root, built_at);
    if n > 0 {
        eprintln!("# ⚠ index が古い: {root} で {n} ファイルが index 後に更新。`kenning update {db_path}` で最新に。");
    }
}

/// index を read-only で開く。無い/壊れているときは backtrace でなく親切に落とす。
fn open_ro(db_path: &str) -> Option<Database> {
    match Database::open_readonly(db_path) {
        Ok(db) => {
            warn_if_stale(&db, db_path);
            Some(db)
        }
        Err(e) => {
            eprintln!("# index を開けない ({db_path}): {e}");
            eprintln!("# 先に: kenning index <dir> {db_path}");
            None
        }
    }
}

/// ファイル eid → path のキャッシュ (sym の所在解決に使う)。
fn file_paths(file_t: &Table) -> HashMap<EntityId, String> {
    let mut m = HashMap::new();
    for e in file_t.all().find().unwrap() {
        m.insert(e, txt(file_t.entity(e).get("path")));
    }
    m
}

/// sym 1 行を `path:line<TAB>vis [async] kind Qual::name  (crate) [#test]` に整形。
fn fmt_sym(sym_t: &Table, paths: &HashMap<EntityId, String>, eid: EntityId) -> String {
    let er = sym_t.entity(eid);
    let name = txt(er.get("name"));
    let container = txt(er.get("container"));
    let qual = if container.is_empty() { name } else { format!("{container}::{name}") };
    let kind = kind_name(num(er.get("kind")));
    let vis = vis_name(num(er.get("vis")));
    let asy = if num(er.get("is_async")) == 1 { "async " } else { "" };
    let tst = if num(er.get("is_test")) == 1 { "  #test" } else { "" };
    let crate_ = txt(er.get("crate_"));
    let line = num(er.get("line"));
    let path = paths.get(&ref_of(er.get("file"))).map(String::as_str).unwrap_or("?");
    format!("{path}:{line}\t{vis} {asy}{kind} {qual}  ({crate_}){tst}")
}

/// sym 群を path:line 昇順で出力 (limit 超は件数を明示)。with_sig でシグネチャも (hover 相当)。
/// 表示用に source 1 行を整形 (trim + 長行 cap。行末の見切れは … で明示)。
const SRC_LINE_MAX: usize = 110;
fn trim_src(s: &str) -> String {
    let t = s.trim();
    if t.chars().count() > SRC_LINE_MAX {
        format!("{}…", t.chars().take(SRC_LINE_MAX).collect::<String>())
    } else {
        t.to_string()
    }
}

/// query 時の source 行 lookup (best-effort 表示用)。file 単位で読んで cache。
/// index 時と path の前提が食い違う (相対 index で cwd が違う等) 場合は黙って "" —
/// path:line は既に出ているので、本文はあくまで「読みに行く手間を省く」おまけ。
struct SrcLines {
    files: HashMap<String, Option<Vec<String>>>,
}
impl SrcLines {
    fn new() -> Self {
        Self { files: HashMap::new() }
    }
    fn line(&mut self, path: &str, ln: u32) -> String {
        let lines = self.files.entry(path.to_string()).or_insert_with(|| {
            std::fs::read_to_string(path).ok().map(|s| s.lines().map(str::to_string).collect())
        });
        match lines {
            Some(v) if ln >= 1 => v.get(ln as usize - 1).map(|s| trim_src(s)).unwrap_or_default(),
            _ => String::new(),
        }
    }
}

/// "path:line\tin caller" 行に source 本文を付ける (取れなければそのまま)。
fn append_src(base: String, src: String) -> String {
    if src.is_empty() { base } else { format!("{base}\t{src}") }
}

/// 定義行 start (1-indexed) の直上に連なる doc コメント / attr 行まで上に広げた開始行を返す。
/// `read` が `#[derive(…)]` や `///` を定義本体と一緒に見せるため。
fn extend_up(lines: &[&str], start: usize) -> usize {
    let mut s = start;
    while s > 1 {
        let t = lines.get(s - 2).map(|l| l.trim_start()).unwrap_or("");
        if t.starts_with("///") || t.starts_with("#[") {
            s -= 1;
        } else {
            break;
        }
    }
    s
}

fn print_syms(sym_t: &Table, paths: &HashMap<EntityId, String>, eids: &[EntityId], limit: usize, with_sig: bool) {
    let mut rows: Vec<(String, u32, EntityId)> = eids
        .iter()
        .map(|&e| {
            let er = sym_t.entity(e);
            (paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(), num(er.get("line")), e)
        })
        .collect();
    rows.sort();
    for (_, _, e) in rows.iter().take(limit) {
        let mut line = fmt_sym(sym_t, paths, *e);
        if with_sig {
            let er = sym_t.entity(*e);
            let sig = txt(er.get("sig"));
            if !sig.is_empty() {
                line = format!("{line}\t{sig}");
            }
            let doc = txt(er.get("doc"));
            if !doc.is_empty() {
                line = format!("{line}\t/// {doc}");
            }
        }
        println!("{line}");
    }
    if rows.len() > limit {
        println!("… (+{} 件省略、--limit {} で全部)", rows.len() - limit, rows.len());
    }
}

/// `--db <path>` / `--limit <n>` を抜き取り、残りを位置引数として返す。
/// db 未指定なら cwd の repo root から自動導出し、無ければ auto-index・古ければ auto-update まで
/// ここで済ませる (儀式ゼロ: クエリ側は db 管理を考えない)。KENNING_NO_AUTO=1 で魔法を全部止める。
struct Opts {
    db: String,
    limit: usize,
    pos: Vec<String>,
}
fn parse_opts(args: &[String]) -> Opts {
    let mut db = std::env::var("KENNING_DB").ok().filter(|s| !s.is_empty());
    let mut limit = 50usize;
    let mut pos = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => { i += 1; if let Some(v) = args.get(i) { db = Some(v.clone()); } }
            "--limit" => { i += 1; if let Some(v) = args.get(i) { limit = v.parse().unwrap_or(50); } }
            other => pos.push(other.to_string()),
        }
        i += 1;
    }
    let no_auto = std::env::var_os("KENNING_NO_AUTO").is_some();
    // 明示 db (--db / env) はそのまま尊重。未指定なら cwd から導出。
    let (db, auto_root) = match db {
        Some(d) => (d, None),
        None => {
            let Some(root) = repo_root_of(".") else {
                eprintln!("# repo root が見つからない (cwd に .git / Cargo.toml の祖先なし)。");
                eprintln!("# repo 内で実行するか、--db <path> / env KENNING_DB で指定を。");
                std::process::exit(2);
            };
            let Some(d) = auto_db_path(&root) else {
                eprintln!("# ~/.cache/kenning を用意できない。--db <path> で指定を。");
                std::process::exit(2);
            };
            (d, Some(root))
        }
    };
    if !no_auto {
        // auto-index: 導出 db が無ければこの場で作る (root 既知の時のみ。明示 db は誤爆防止で作らない)。
        if !std::path::Path::new(&db).exists() {
            if let Some(root) = &auto_root {
                eprintln!("# index が無い → 自動 full index: {} → {}", root.display(), db);
                run_index(&root.to_string_lossy(), &db, None);
                STALE_CHECKED.store(true, Ordering::Relaxed); // 今作ったばかり = 最新
            }
        } else {
            maybe_auto_update(&db); // auto-update: 古ければ増分してから答える
        }
    }
    Opts { db, limit, pos }
}

/// index が古ければ増分 update してから返る。lock が取れなければ古いまま警告 (安全側)。
/// stat-walk のみなので通常コストは数 ms。KENNING_NO_STALE=1 でスキップ。
fn maybe_auto_update(db_path: &str) {
    if std::env::var_os("KENNING_NO_STALE").is_some() {
        return;
    }
    // ここで鮮度は確認 (必要なら更新) される。lock 失敗時も警告は自前で出すので、
    // どの経路でも open_ro 側 warn_if_stale の再 walk は不要。
    STALE_CHECKED.store(true, Ordering::Relaxed);
    let (root, built_at, ver) = {
        let Ok(db) = Database::open_readonly(db_path) else { return };
        let Some((r, b)) = read_meta(&db) else { return };
        let v = db
            .get_table("meta")
            .and_then(|t| t.all().find().unwrap().into_iter().next().map(|e| match t.entity(e).get("ver") {
                Some(Value::Number(n)) => n as u32,
                _ => 0,
            }))
            .unwrap_or(0);
        (r, b, v)
    }; // ← readonly を閉じてから書込 open する
    let ver_old = ver != INDEX_VER; // 版違い: ファイル無変更でも full 再 index へ (update_inner が panic → heal)
    if root.is_empty() || built_at == 0 || (!ver_old && count_stale(&root, built_at) == 0) {
        return;
    }
    match Database::open(db_path) {
        Ok(db) => {
            if ver_old {
                eprintln!("# index の版が古い (v{ver} → v{INDEX_VER}) → full 再 index ({root})");
            } else {
                eprintln!("# index が古い → 自動増分 update ({root})");
            }
            update_with_heal(db, &root, db_path); // 旧 schema/旧版なら full 再 index で自己修復
        }
        Err(e) => {
            eprintln!("# ⚠ index が古いが lock を取れない ({e}) → 古い結果で回答。後で `kenning update` を。");
        }
    }
}

/// `def <name>` — 名前の定義位置。grep の全マッチではなく sym の等値一撃。
pub fn cmd_def(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning def <name> [--db P] [--limit N]");
        return;
    };
    run_search(&o.db, &[format!("name:{name}")], o.limit, true); // def = hover 相当で sig も
}

/// `read <name> [container]` — 定義本体をそのまま出す (`def` → Read の 2 手を 1 手に)。
/// Read tool と違い「その item の範囲だけ」なので token も節約。範囲は index 済みの
/// line..end_line + 直上の doc/attr 行 (query 時に上へ拡張)。
pub fn cmd_read(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning read <name> [container] [--db P]");
        return;
    };
    run_read(&o.db, name, o.pos.get(1).map(String::as_str), o.limit);
}

/// 一度に出す本体の上限行数。超える item は頭からここまで + 続きの Read 案内 (暴発防止)。
const READ_MAX_LINES: usize = 400;

fn run_read(db_path: &str, name: &str, container: Option<&str>, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let paths = file_paths(&file_t);

    let defs = defs_of(&sym_t, name, container);
    if defs.is_empty() {
        println!("# \"{name}\" の定義が index に無い{}。", container.map(|c| format!(" (container={c})")).unwrap_or_default());
        suggest_similar(&sym_t, &paths, name);
        return;
    }
    if defs.len() > 1 {
        println!("# \"{name}\" は {} 定義 (同名)。`read {name} <container>` で絞る:", defs.len());
        print_syms(&sym_t, &paths, &defs, limit, true);
        return;
    }
    let er = sym_t.entity(defs[0]);
    let path = paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default();
    let start = num(er.get("line")) as usize;
    let end = (num(er.get("end_line")) as usize).max(start);
    println!("{}", fmt_sym(&sym_t, &paths, defs[0]));
    let Ok(src) = std::fs::read_to_string(&path) else {
        println!("# source を読めない: {path} (index 時と cwd が違う? full path で index を)");
        return;
    };
    let lines: Vec<&str> = src.lines().collect();
    let s = extend_up(&lines, start);
    let shown_end = end.min(s + READ_MAX_LINES - 1);
    for (i, l) in lines.iter().enumerate().take(shown_end).skip(s - 1) {
        println!("{:>5}\t{l}", i + 1);
    }
    if shown_end < end {
        println!("# … 長大なので {READ_MAX_LINES} 行で打ち切り (+{} 行)。続き: Read {path} offset={}", end - shown_end, shown_end + 1);
    }
}

/// `find <substr>` — 名前の部分一致 (大文字小文字無視)。exact な `def` の補完 = 名前の発見用。
/// tag 等値では引けないので sym 全走査 (数千件なので µs–ms)。
pub fn cmd_find(args: &[String]) {
    let o = parse_opts(args);
    let Some(needle) = o.pos.first() else {
        eprintln!("usage: kenning find <substr> [--db P] [--limit N]");
        return;
    };
    let needle = needle.to_lowercase();
    let Some(db) = open_ro(&o.db) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let paths = file_paths(&file_t);
    let hits: Vec<EntityId> = sym_t
        .all()
        .find()
        .unwrap()
        .into_iter()
        .filter(|&e| txt(sym_t.entity(e).get("name")).to_lowercase().contains(&needle))
        .collect();
    println!("# {} symbols  [name ~ \"{needle}\"]", hits.len());
    print_syms(&sym_t, &paths, &hits, o.limit, false);
}

/// `search kind:fn vis:pub crate:… container:… async:1 …` — faceted 等値 AND。
pub fn cmd_search(args: &[String]) {
    let o = parse_opts(args);
    if o.pos.is_empty() {
        eprintln!("usage: kenning search <facet...>  例: kind:method vis:pub calls:unwrap");
        eprintln!("  facet: name: kind:(fn|method|struct|enum|trait|const) vis:(pub|crate|restricted|priv)");
        eprintln!("         async:(0|1) test:(0|1) crate: container: module:");
        eprintln!("         calls:<name>  (本体で <name> を呼ぶ sym に絞る = grep 不可の edge×facet AND)");
        return;
    }
    run_search(&o.db, &o.pos, o.limit, false);
}

fn run_search(db_path: &str, facets: &[String], limit: usize, with_sig: bool) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    let mut q = sym_t.all();
    let mut applied: Vec<String> = Vec::new();
    let mut calls_filters: Vec<String> = Vec::new(); // calls:X = 本体で X を呼ぶ sym に絞る (edge facet)
    for f in facets {
        let Some((k, v)) = f.split_once(':') else {
            eprintln!("# 無視: \"{f}\" (key:value 形式で)");
            continue;
        };
        match k {
            "name" => { q = q.where_eq("name", v); applied.push(format!("name={v}")); }
            "kind" => match kind_code(v) {
                Some(c) => { q = q.where_eq("kind", c); applied.push(format!("kind={v}")); }
                None => eprintln!("# 無視: 未知 kind \"{v}\" (fn/method/struct/enum/trait/const)"),
            },
            "vis" => match vis_code(v) {
                Some(c) => { q = q.where_eq("vis", c); applied.push(format!("vis={v}")); }
                None => eprintln!("# 無視: 未知 vis \"{v}\" (pub/crate/restricted/priv)"),
            },
            "async" => { q = q.where_eq("is_async", bool01(v)); applied.push(format!("async={v}")); }
            "test" => { q = q.where_eq("is_test", bool01(v)); applied.push(format!("test={v}")); }
            "crate" => { q = q.where_eq("crate_", v); applied.push(format!("crate={v}")); }
            "container" => { q = q.where_eq("container", v); applied.push(format!("container={v}")); }
            "module" => { q = q.where_eq("module", v); applied.push(format!("module={v}")); }
            "calls" => { calls_filters.push(v.to_string()); applied.push(format!("calls={v}")); }
            _ => eprintln!("# 無視: 未知 facet key \"{k}\""),
        }
    }
    let mut hits = q.find().unwrap();
    // edge facet: 本体が X を呼ぶ sym だけ残す (grep には表現できない sym facet × call edge の AND)。
    for cn in &calls_filters {
        let callers: HashSet<EntityId> = call_t
            .where_eq("callee", cn.as_str())
            .find()
            .unwrap()
            .into_iter()
            .map(|c| ref_of(call_t.entity(c).get("caller")))
            .collect();
        hits.retain(|e| callers.contains(e));
    }
    println!("# {} symbols  [{}]", hits.len(), applied.join(" "));
    print_syms(&sym_t, &paths, &hits, limit, with_sig);
}

/// `callers <name> [container]` — 精密 who-calls (名前解決した callee_sym の eid 逆引き)。
pub fn cmd_callers(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning callers <name> [container] [--db P] [--limit N]");
        return;
    };
    let container = o.pos.get(1).map(String::as_str);
    run_callers(&o.db, name, container, o.limit);
}

fn run_callers(db_path: &str, name: &str, container: Option<&str>, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    let mut defs = sym_t.where_eq("name", name).find().unwrap();
    if let Some(c) = container {
        defs.retain(|&e| txt(sym_t.entity(e).get("container")) == c);
    }
    let name_total = call_t.where_eq("callee", name).count().unwrap();
    if defs.is_empty() {
        println!("# \"{name}\" の定義が index に無い。名前一致の call = {name_total} 件 (外部/未解決)");
        suggest_similar(&sym_t, &paths, name);
        return;
    }

    // 同名定義が多いと caller を全部出すと冗長。container 指定が無く定義が複数なら
    // まず「定義ごとの精密 caller 数」の要約表だけ出して、絞り方を促す。
    if container.is_none() && defs.len() > 1 {
        let mut rows: Vec<(usize, EntityId)> = defs
            .iter()
            .map(|&d| (call_t.where_eq("callee_sym", Value::Ref(d)).count().unwrap(), d))
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0)); // caller 数 降順
        let precise_sum: usize = rows.iter().map(|(n, _)| n).sum();
        println!("# \"{name}\" は {} 型が定義 (同名)。定義ごとの精密 caller 数:", defs.len());
        for (n, d) in rows.iter().take(limit) {
            let er = sym_t.entity(*d);
            let ct = txt(er.get("container"));
            let path = paths.get(&ref_of(er.get("file"))).map(String::as_str).unwrap_or("?");
            println!("  {n:>5}  {}::{name}  ({})  {path}:{}", if ct.is_empty() { "·".into() } else { ct }, txt(er.get("crate_")), num(er.get("line")));
        }
        let unresolved = name_total.saturating_sub(precise_sum);
        println!("# 絞る: `callers {name} <container>` (例: {}) — 未確定の候補も位置付きで出る", txt(sym_t.entity(rows[0].1).get("container")));
        if unresolved > 0 {
            println!("# 名前一致 {name_total} 件中 {precise_sum} 件を確定。残り {unresolved} 件は未確定(候補、drill-in で位置表示)。");
        }
        return;
    }

    // 名前一致する全 call を 1 度引く (確実/候補の切り分けに使う)。
    let name_matches = call_t.where_eq("callee", name).find().unwrap();
    // callee ident をラベル整形するクロージャ (caller sym → "Container::name")。
    let caller_label = |c: EntityId| -> (String, u32, String) {
        let er = call_t.entity(c);
        let p = paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default();
        let ln = num(er.get("line"));
        let cr = sym_t.entity(ref_of(er.get("caller")));
        let ct = txt(cr.get("container"));
        let nm = txt(cr.get("name"));
        let cq = if ct.is_empty() { nm } else { format!("{ct}::{nm}") };
        (p, ln, cq)
    };

    let mut src = SrcLines::new(); // 呼び出し行の本文 (「どう呼ばれてるか」を read-back なしで)
    let mut precise_sum = 0usize;
    for &d in &defs {
        println!("{}", fmt_sym(&sym_t, &paths, d));
        let calls = call_t.where_eq("callee_sym", Value::Ref(d)).find().unwrap();
        precise_sum += calls.len();
        let mut rows: Vec<(String, u32, String)> = calls.iter().map(|&c| caller_label(c)).collect();
        rows.sort();
        println!("  ← {} 確実 callers (callee_sym 逆引き、誤りなし):", rows.len());
        for (p, ln, cq) in rows.iter().take(limit) {
            println!("{}", append_src(format!("    {p}:{ln}\tin {cq}"), src.line(p, *ln)));
        }
        if rows.len() > limit {
            println!("    … (+{} 件省略)", rows.len() - limit);
        }
    }

    // ── 候補 (completeness backstop): 名前一致だが callee_sym 未 set (型推論待ち/cfg 非活性/
    //    外部同名)。確実集合の「見逃し」がここに全部いる = grep superset の未確認部分。これを
    //    出すことで Claude は「本当に全 caller を掴んだか」を grep に戻らず目視できる。
    let mut cand: Vec<(String, u32, String, u32)> = name_matches
        .iter()
        .copied()
        .filter(|&c| !matches!(call_t.entity(c).get("callee_sym"), Some(Value::Ref(_))))
        .map(|c| {
            let (p, ln, cq) = caller_label(c);
            (p, ln, cq, num(call_t.entity(c).get("res")))
        })
        .collect();
    cand.sort();
    let other = name_total.saturating_sub(precise_sum).saturating_sub(cand.len());
    if !cand.is_empty() {
        println!(
            "  ⚠ {} 候補 (名前一致だが未確定 — 要確認、確実集合の漏れはここに全部):",
            cand.len()
        );
        for (p, ln, cq, res) in cand.iter().take(limit) {
            let base = format!("    {p}:{ln}\tin {cq}  [{}]", RES_NAMES.get(*res as usize).unwrap_or(&"?"));
            println!("{}", append_src(base, src.line(p, *ln)));
        }
        if cand.len() > limit {
            println!("    … (+{} 件省略、--limit で全部)", cand.len() - limit);
        }
    }
    println!(
        "# 名前一致 {name_total} = 確実 {precise_sum} + 候補未確定 {} + 別の同名 sym に確定 {other}",
        cand.len()
    );
}

/// `edges` — 解決済み call edge をファイル間で集計して一括出力。
/// 行形式: `caller_path<TAB>callee_path<TAB>count`（cross-file のみ、path 昇順、stdout 純 TSV）。
/// モジュール/パッケージ単位の依存グラフを外部ツール (可視化 GUI 等) が組むための素材で、
/// per-symbol の callers/callees と違い repo 全体を 1 回で吐く。
pub fn cmd_edges(args: &[String]) {
    let o = parse_opts(args);
    run_edges(&o.db);
}

fn run_edges(db_path: &str) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    // callee 側は「定義があるファイル」= sym.file を引く
    let mut sym_file: HashMap<EntityId, EntityId> = HashMap::new();
    for e in sym_t.all().find().unwrap() {
        sym_file.insert(e, ref_of(sym_t.entity(e).get("file")));
    }

    let mut counts: HashMap<(String, String), usize> = HashMap::new();
    let mut total = 0usize;
    for c in call_t.all().find().unwrap() {
        let er = call_t.entity(c);
        // callee_sym 未 set = 外部 crate / 未解決 → 依存グラフの素材にならないので除外
        let Some(Value::Ref(target)) = er.get("callee_sym") else { continue };
        let from_f = ref_of(er.get("file"));
        let Some(&to_f) = sym_file.get(&target) else { continue };
        if from_f == to_f {
            continue; // 同一ファイル内呼び出しはファイル間依存ではない
        }
        let (Some(fp), Some(tp)) = (paths.get(&from_f), paths.get(&to_f)) else { continue };
        *counts.entry((fp.clone(), tp.clone())).or_default() += 1;
        total += 1;
    }

    let mut rows: Vec<((String, String), usize)> = counts.into_iter().collect();
    rows.sort();
    for ((f, t), n) in &rows {
        println!("{f}\t{t}\t{n}");
    }
    eprintln!("# {} file-pair edges ({total} resolved cross-file calls)", rows.len());
}

/// `callees <name> [container]` — X が呼ぶ先 (outgoing calls、`callers` の鏡)。
/// rust-analyzer の callHierarchy/outgoingCalls 相当。「この fn は何に依存するか」。
pub fn cmd_callees(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning callees <name> [container] [--db P] [--limit N]");
        return;
    };
    run_callees(&o.db, name, o.pos.get(1).map(String::as_str), o.limit);
}

fn run_callees(db_path: &str, name: &str, container: Option<&str>, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);
    let defs = defs_of(&sym_t, name, container);
    if defs.is_empty() {
        println!("# \"{name}\" の定義が index に無い。");
        suggest_similar(&sym_t, &paths, name);
        return;
    }
    for &d in &defs {
        println!("{}", fmt_sym(&sym_t, &paths, d));
        let calls = call_t.where_eq("caller", Value::Ref(d)).find().unwrap();
        // callee_sym が set = workspace 定義に確定、未 set = 外部/std/型推論待ち。
        let mut resolved: HashMap<EntityId, usize> = HashMap::new();
        let mut external: HashMap<String, usize> = HashMap::new();
        for &c in &calls {
            match call_t.entity(c).get("callee_sym") {
                Some(Value::Ref(t)) => *resolved.entry(t).or_default() += 1,
                _ => *external.entry(txt(call_t.entity(c).get("callee"))).or_default() += 1,
            }
        }
        let mut rows: Vec<(String, u32, EntityId, usize)> = resolved
            .iter()
            .map(|(&t, &n)| {
                let er = sym_t.entity(t);
                (paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(), num(er.get("line")), t, n)
            })
            .collect();
        rows.sort();
        println!("  → {} 確定 callees (呼ぶ先 workspace 定義、path:line = 定義位置):", rows.len());
        for (p, ln, t, n) in rows.iter().take(limit) {
            println!("    {p}:{ln}\t→ {}  (×{n})", sym_qual(&sym_t, *t));
        }
        if rows.len() > limit {
            println!("    … (+{} 件省略)", rows.len() - limit);
        }
        if !external.is_empty() {
            let mut ext: Vec<(&String, &usize)> = external.iter().collect();
            ext.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0))); // 呼び回数 降順
            let shown: Vec<String> = ext.iter().take(12).map(|(n, c)| format!("{n}(×{c})")).collect();
            println!(
                "  → 外部/未解決 {} 種 (std/dep/型推論待ち): {}{}",
                external.len(), shown.join(", "), if external.len() > 12 { " …" } else { "" }
            );
        }
    }
}

/// `impls <name>` — go-to-implementation。name が trait なら実装型、型なら実装 trait を出す。
pub fn cmd_impls(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning impls <trait|type> [--db P] [--limit N]");
        return;
    };
    run_impls(&o.db, name, o.limit);
}

fn run_impls(db_path: &str, name: &str, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let Some(impl_t) = db.get_table("impl") else {
        println!("# impl 情報が無い (旧 index)。`kenning index <dir>` で再 index すると使える。");
        return;
    };
    let paths = file_paths(&file_t);
    // path:line<TAB>相手名 で列挙するヘルパ (col = 出す相手の列名)。
    let dump = |eids: &[EntityId], col: &str| {
        let mut rows: Vec<(String, u32, String)> = eids
            .iter()
            .map(|&e| {
                let er = impl_t.entity(e);
                (paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(), num(er.get("line")), txt(er.get(col)))
            })
            .collect();
        rows.sort();
        for (p, ln, n) in rows.iter().take(limit) {
            println!("  {p}:{ln}\t{n}");
        }
        if rows.len() > limit {
            println!("  … (+{} 件省略、--limit で全部)", rows.len() - limit);
        }
    };
    let as_trait = impl_t.where_eq("trait_name", name).find().unwrap();
    let as_type = impl_t.where_eq("type_name", name).find().unwrap();
    if as_trait.is_empty() && as_type.is_empty() {
        println!("# \"{name}\" の impl 関係が index に無い (trait/型でない、or impl が外部・cfg 非活性)。");
        return;
    }
    if !as_trait.is_empty() {
        println!("# trait \"{name}\" を実装する型 ({}):", as_trait.len());
        dump(&as_trait, "type_name");
    }
    if !as_type.is_empty() {
        println!("# 型 \"{name}\" が実装する trait ({}):", as_type.len());
        dump(&as_type, "trait_name");
    }
}

/// `text <term>` — 全文検索 (コメント/文字列も) + **enclosing symbol 注釈**。
/// grep superset with structure: どの関数の中のヒットかが 1 行で分かる = 追い Read を 1 個消す。
/// 検索対象は index 済みファイル (live に読む = 常に最新)。大小無視。
pub fn cmd_text(args: &[String]) {
    let o = parse_opts(args);
    let Some(needle) = o.pos.first() else {
        eprintln!("usage: kenning text <term> [--db P] [--limit N]");
        return;
    };
    let needle_l = needle.to_lowercase();
    let Some(db) = open_ro(&o.db) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();

    // file eid → その file の sym (line 昇順)。enclosing = hit 行以前で最後の定義 (end 無しの近似)。
    let mut syms_by_file: HashMap<EntityId, Vec<(u32, EntityId)>> = HashMap::new();
    for s in sym_t.all().find().unwrap() {
        let er = sym_t.entity(s);
        syms_by_file.entry(ref_of(er.get("file"))).or_default().push((num(er.get("line")), s));
    }
    for v in syms_by_file.values_mut() {
        v.sort();
    }

    let mut files: Vec<(String, EntityId)> =
        file_t.all().find().unwrap().into_iter().map(|e| (txt(file_t.entity(e).get("path")), e)).collect();
    files.sort();
    let mut shown = 0usize;
    let mut total = 0usize;
    for (path, fe) in &files {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        if !src.to_lowercase().contains(&needle_l) {
            continue; // ファイル単位の早期スキップ
        }
        let syms = syms_by_file.get(fe);
        for (i, line) in src.lines().enumerate() {
            if !line.to_lowercase().contains(&needle_l) {
                continue;
            }
            total += 1;
            if shown >= o.limit {
                continue; // 件数は数え続ける
            }
            shown += 1;
            let ln = (i + 1) as u32;
            let text = line.trim();
            // enclosing symbol: 通常は「line <= ln の最後の定義」の中。`///` は次の定義の doc、
            // `//!` はモジュール doc (どの item のものでもない)。end 無しの近似。
            let is_mod_doc = text.starts_with("//!");
            let is_doc = text.starts_with("///");
            let encl = if is_mod_doc {
                None
            } else {
                syms.and_then(|v| {
                    let idx = v.partition_point(|(l, _)| *l <= ln);
                    if is_doc {
                        v.get(idx).map(|&(_, e)| (e, "doc of"))
                    } else if idx > 0 {
                        Some((v[idx - 1].1, "in"))
                    } else {
                        None
                    }
                })
                .map(|(e, rel)| (sym_qual(&sym_t, e), rel))
                .filter(|(q, _)| !q.is_empty())
            };
            let text: String = if text.chars().count() > 120 { text.chars().take(120).collect::<String>() + "…" } else { text.to_string() };
            match encl {
                Some((q, rel)) => println!("{path}:{ln}\t{text}\t({rel} {q})"),
                None if is_mod_doc => println!("{path}:{ln}\t{text}\t(module doc)"),
                None => println!("{path}:{ln}\t{text}"),
            }
        }
    }
    if total > shown {
        println!("… (+{} 件省略、--limit {total} で全部)", total - shown);
    }
    if total == 0 {
        println!("# \"{needle}\" は index 済みファイルに無い (対象は .rs のみ。他は grep で)");
    }
}

/// `across <name>` — **全 repo 横断**: cache 内の全 repo db を走査し、name の定義・利用を鳥瞰する。
/// さらに SCIP symbol (グローバル一意) を extref と突き合わせ、**repo を跨いだ精密参照**
/// (例: 利用側 repo が enchudb::finish_with_oplog を使う行) を出す。RA は single-workspace なので出せない問い。
pub fn cmd_across(args: &[String]) {
    // db は使わない (cache 全走査) ので parse_opts の auto 魔法は通さない。
    let mut limit = 20usize;
    let mut name: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--limit" => { i += 1; if let Some(v) = args.get(i) { limit = v.parse().unwrap_or(20); } }
            other => name = Some(other.to_string()),
        }
        i += 1;
    }
    let Some(name) = name else {
        eprintln!("usage: kenning across <name> [--limit N]");
        return;
    };
    let Ok(home) = std::env::var("HOME") else { return };
    let cache = std::path::Path::new(&home).join(".cache/kenning");
    let mut dbs: Vec<String> = std::fs::read_dir(&cache)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path().to_string_lossy().to_string())
                .filter(|p| p.ends_with(".db"))
                .collect()
        })
        .unwrap_or_default();
    dbs.sort();
    if dbs.is_empty() {
        println!("# cache に index が無い (~/.cache/kenning)。各 repo で一度 kenning を叩くと増える。");
        return;
    }

    // pass1: repo ごとの定義 / 確実 callers / 名前一致。定義の SCIP symbol を集める (pass2 の鍵)。
    let mut symbols: Vec<String> = Vec::new(); // 定義側のグローバル一意 symbol
    let mut opened = 0usize;
    println!("# across \"{name}\" — {} repo index を走査:", dbs.len());
    for dbp in &dbs {
        let Ok(db) = Database::open_readonly(dbp) else { continue };
        opened += 1;
        let repo = read_meta(&db)
            .map(|(r, _)| r.rsplit('/').next().unwrap_or(&r).to_string())
            .unwrap_or_else(|| dbp.rsplit('/').next().unwrap_or(dbp).to_string());
        let (Some(file_t), Some(sym_t), Some(call_t)) = (db.get_table("file"), db.get_table("sym"), db.get_table("call")) else { continue };
        let defs = sym_t.where_eq("name", name.as_str()).find().unwrap_or_default();
        let name_calls = call_t.where_eq("callee", name.as_str()).count().unwrap_or(0);
        if defs.is_empty() && name_calls == 0 {
            continue;
        }
        let paths = file_paths(&file_t);
        let mut precise = 0usize;
        for &d in &defs {
            precise += call_t.where_eq("callee_sym", Value::Ref(d)).count().unwrap_or(0);
            let s = txt(sym_t.entity(d).get("symbol"));
            if !s.is_empty() {
                symbols.push(s);
            }
        }
        println!("{repo}: 定義 {} / 確実 callers {} / 名前一致 call {}", defs.len(), precise, name_calls);
        for &d in defs.iter().take(3) {
            println!("  {}", fmt_sym(&sym_t, &paths, d));
        }
    }

    // pass2: 定義 symbol を全 repo の extref と突き合わせ = repo を跨いだ精密参照。
    symbols.sort();
    symbols.dedup();
    if !symbols.is_empty() {
        let mut n_x = 0usize;
        let mut shown = 0usize;
        for dbp in &dbs {
            let Ok(db) = Database::open_readonly(dbp) else { continue };
            let Some(extref_t) = db.get_table("extref") else { continue };
            let Some(file_t) = db.get_table("file") else { continue };
            let repo = read_meta(&db)
                .map(|(r, _)| r.rsplit('/').next().unwrap_or(&r).to_string())
                .unwrap_or_default();
            let paths = file_paths(&file_t);
            for s in &symbols {
                for r in extref_t.where_eq("symbol", s.as_str()).find().unwrap_or_default() {
                    n_x += 1;
                    if shown >= limit {
                        continue;
                    }
                    shown += 1;
                    let er = extref_t.entity(r);
                    let p = paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default();
                    println!("  ✕ {repo} → {name}: {p}:{}\t[{}]", num(er.get("line")), role_name(num(er.get("role"))));
                }
            }
        }
        if n_x > 0 {
            println!("# repo 跨ぎ精密参照 (extref×SCIP symbol): {n_x} 件");
            if n_x > shown {
                println!("  … (+{} 件省略)", n_x - shown);
            }
        } else {
            println!("# repo 跨ぎ精密参照: 0 件 (利用側 repo が bake 済みの時だけ出る)");
        }
    }
    eprintln!("# ({opened}/{} db を走査。各 repo の鮮度は個別 query 時に自動 update)", dbs.len());
}

/// SCIP role bit → 表示名。
fn role_name(r: u32) -> &'static str {
    if r & 1 != 0 { "def" }
    else if r & 4 != 0 { "write" }
    else if r & 2 != 0 { "import" }
    else if r & 8 != 0 { "read" }
    else { "ref" }
}

/// `refs <name> [container]` — 正確な find-all-refs (SCIP 全 occurrence の逆引き、要 --scip index)。
/// who-calls の上位互換: 呼び出しだけでなく読み/書き/型参照も含む全参照。
pub fn cmd_refs(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning refs <name> [container] [--db P] [--limit N]");
        return;
    };
    let container = o.pos.get(1).map(String::as_str);
    let Some(db) = open_ro(&o.db) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let ref_t = db.get_table("ref").unwrap();
    let paths = file_paths(&file_t);

    if ref_t.all().count().unwrap() == 0 {
        println!("# ref table が空 = SCIP 無しで index された。`index <dir> --scip <f>` で正確 refs が使える。");
        return;
    }

    let mut defs = sym_t.where_eq("name", name).find().unwrap();
    if let Some(c) = container {
        defs.retain(|&e| txt(sym_t.entity(e).get("container")) == c);
    }
    if defs.is_empty() {
        println!("# \"{name}\" の定義が index に無い。");
        suggest_similar(&sym_t, &paths, name);
        return;
    }

    // 同名多数 & container 未指定 → 定義ごとの参照数の要約 + 絞り方。
    if container.is_none() && defs.len() > 1 {
        let mut rows: Vec<(usize, EntityId)> = defs
            .iter()
            .map(|&d| (ref_t.where_eq("symbol_sym", Value::Ref(d)).count().unwrap(), d))
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        println!("# \"{name}\" は {} 型が定義。定義ごとの参照数:", defs.len());
        for (n, d) in rows.iter().take(o.limit) {
            let er = sym_t.entity(*d);
            let ct = txt(er.get("container"));
            println!("  {n:>5}  {}::{name}  ({})", if ct.is_empty() { "·".into() } else { ct }, txt(er.get("crate_")));
        }
        println!("# 絞る: `refs {name} <container>`");
        return;
    }

    for &d in &defs {
        println!("{}", fmt_sym(&sym_t, &paths, d));
        let refs = ref_t.where_eq("symbol_sym", Value::Ref(d)).find().unwrap();
        let mut rows: Vec<(String, u32, u32)> = refs
            .iter()
            .map(|&r| {
                let er = ref_t.entity(r);
                (paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(), num(er.get("line")), num(er.get("role")))
            })
            .collect();
        rows.sort();
        println!("  {} refs:", rows.len());
        for (p, ln, role) in rows.iter().take(o.limit) {
            println!("    {p}:{ln}\t[{}]", role_name(*role));
        }
        if rows.len() > o.limit {
            println!("    … (+{} 件省略)", rows.len() - o.limit);
        }
    }
    // refs は SCIP 確定のみ = 精密だが cfg 非活性/未解析域は落ちる。superset が要るなら find/grep。
    println!("# SCIP 確定参照のみ (誤りなし)。cfg 非活性/未解析域は含まない → superset は `find {name}` / grep。");
}

// ─────────────────────────── 多段 graph (推移的 callers / 呼び出し経路) ───────────────────────────
// call table = caller(sym) → callee_sym(sym) の有向 edge。確定 edge (callee_sym set) だけを辿る。
// grep も単発 LSP も出せない「推移的な影響範囲 / 経路」を edge 逆引き µs で出す。

/// 名前が index に無い時の発見導線: 近い定義名を数件提案する。
/// `foo_bar` を `_` 分割し、長さ 4+ の token を含む定義名を拾う (typo/部分名を救う)。
/// 例: `finish_oplog` → token [finish, oplog] → `finish_with_oplog` が引っかかる。
fn suggest_similar(sym_t: &Table, paths: &HashMap<EntityId, String>, name: &str) {
    let lname = name.to_lowercase();
    let tokens: Vec<&str> = lname.split(|c: char| c == '_' || !c.is_alphanumeric()).filter(|t| t.len() >= 4).collect();
    // 各定義を「一致 token 数」でスコア。substring 全体一致も 1 点上乗せ。
    let mut best: HashMap<String, (u32, EntityId)> = HashMap::new(); // name → (score, 代表 eid)
    for e in sym_t.all().find().unwrap() {
        let dn = txt(sym_t.entity(e).get("name"));
        let ldn = dn.to_lowercase();
        let mut score = tokens.iter().filter(|t| ldn.contains(**t)).count() as u32;
        if ldn.contains(&lname) || lname.contains(&ldn) {
            score += 1;
        }
        if score > 0 {
            let slot = best.entry(dn).or_insert((0, e));
            if score > slot.0 { *slot = (score, e); }
        }
    }
    if best.is_empty() {
        println!("# 近い名前なし。`find <部分文字列>` で探せる。");
        return;
    }
    let mut ranked: Vec<(u32, String, EntityId)> = best.into_iter().map(|(n, (s, e))| (s, n, e)).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1))); // score 降順 → 名前昇順
    println!("# \"{name}\" は無い。近い名前:");
    for (_, n, e) in ranked.iter().take(8) {
        let er = sym_t.entity(*e);
        let ct = txt(er.get("container"));
        let qn = if ct.is_empty() { n.clone() } else { format!("{ct}::{n}") };
        let path = paths.get(&ref_of(er.get("file"))).map(String::as_str).unwrap_or("?");
        println!("  {qn}\t{path}:{}", num(er.get("line")));
    }
}

/// name[+container] → 定義 eid 群 (callers/refs/impact/path 共通、DRY)。
fn defs_of(sym_t: &Table, name: &str, container: Option<&str>) -> Vec<EntityId> {
    let mut defs = sym_t.where_eq("name", name).find().unwrap();
    if let Some(c) = container {
        defs.retain(|&e| txt(sym_t.entity(e).get("container")) == c);
    }
    defs
}

/// 逆方向 BFS: 起点 = 全定義。depth 別に確定 caller を層状に集める (impact / tests 共用)。
/// 返り値 [depth1 の層, depth2 の層, …] (起点は含まない)。
fn caller_bfs(call_t: &Table, defs: &[EntityId]) -> Vec<Vec<EntityId>> {
    let mut depth: HashMap<EntityId, u32> = defs.iter().map(|&d| (d, 0)).collect();
    let mut frontier: Vec<EntityId> = defs.to_vec();
    let mut by_depth: Vec<Vec<EntityId>> = Vec::new();
    let mut d = 0u32;
    while !frontier.is_empty() && d < 64 {
        let mut next = Vec::new();
        for &s in &frontier {
            for caller in direct_callers(call_t, s) {
                if let std::collections::hash_map::Entry::Vacant(e) = depth.entry(caller) {
                    e.insert(d + 1);
                    next.push(caller);
                }
            }
        }
        if !next.is_empty() {
            by_depth.push(next.clone());
        }
        frontier = next;
        d += 1;
    }
    by_depth
}

/// s を確定 edge で呼ぶ直接 caller の sym eid 群 (逆方向)。
fn direct_callers(call_t: &Table, s: EntityId) -> Vec<EntityId> {
    call_t
        .where_eq("callee_sym", Value::Ref(s))
        .find()
        .unwrap()
        .into_iter()
        .map(|c| ref_of(call_t.entity(c).get("caller")))
        .collect()
}

/// s が確定 edge で呼ぶ直接 callee の sym eid 群 (前方)。
fn direct_callees(call_t: &Table, s: EntityId) -> Vec<EntityId> {
    call_t
        .where_eq("caller", Value::Ref(s))
        .find()
        .unwrap()
        .into_iter()
        .filter_map(|c| match call_t.entity(c).get("callee_sym") {
            Some(Value::Ref(e)) => Some(e),
            _ => None,
        })
        .collect()
}

/// sym eid 群を (path, line) 昇順で `path:line<TAB>詳細` 出力 (limit 超は件数明示)。
fn print_sym_layer(sym_t: &Table, paths: &HashMap<EntityId, String>, eids: &[EntityId], limit: usize, indent: &str) {
    let mut rows: Vec<(String, u32, EntityId)> = eids
        .iter()
        .map(|&e| (paths.get(&ref_of(sym_t.entity(e).get("file"))).cloned().unwrap_or_default(), num(sym_t.entity(e).get("line")), e))
        .collect();
    rows.sort();
    for (_, _, e) in rows.iter().take(limit) {
        println!("{indent}{}", fmt_sym(sym_t, paths, *e));
    }
    if rows.len() > limit {
        println!("{indent}… (+{} 件省略、--limit で全部)", rows.len() - limit);
    }
}

/// `impact <name> [container]` — 推移的 callers (逆方向 BFS)。「これを変えると壊れる範囲」。
pub fn cmd_impact(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning impact <name> [container] [--db P] [--limit N]");
        return;
    };
    run_impact(&o.db, name, o.pos.get(1).map(String::as_str), o.limit);
}

fn run_impact(db_path: &str, name: &str, container: Option<&str>, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    let defs = defs_of(&sym_t, name, container);
    if defs.is_empty() {
        println!("# \"{name}\" の定義が index に無い{}。", container.map(|c| format!(" (container={c})")).unwrap_or_default());
        suggest_similar(&sym_t, &paths, name);
        return;
    }
    let by_depth = caller_bfs(&call_t, &defs);
    for &def in &defs {
        println!("{}", fmt_sym(&sym_t, &paths, def));
    }
    let total: usize = by_depth.iter().map(|v| v.len()).sum();
    for (i, layer) in by_depth.iter().enumerate() {
        println!("  depth {} ({} sym){}:", i + 1, layer.len(), if i == 0 { " = 直接 callers" } else { "" });
        print_sym_layer(&sym_t, &paths, layer, limit, "    ");
    }
    println!("# 推移的 callers: {total} sym (確定 edge のみ = 影響の下界。候補/未解決 edge は未算入 → `callers` で確認)");
}

/// `tests <name> [container]` — この sym を (推移的に) 呼ぶテスト = impact ∩ is_test。
/// 「これを変えたらどのテストを回すか」を 1 コマンドで。
pub fn cmd_tests(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning tests <name> [container] [--db P] [--limit N]");
        return;
    };
    run_tests(&o.db, name, o.pos.get(1).map(String::as_str), o.limit);
}

/// `cargo test -- <filter...>` のヒントに載せる最大テスト数 (多すぎたら cargo test 全部が早い)。
const TESTS_HINT_MAX: usize = 8;

fn run_tests(db_path: &str, name: &str, container: Option<&str>, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    let defs = defs_of(&sym_t, name, container);
    if defs.is_empty() {
        println!("# \"{name}\" の定義が index に無い{}。", container.map(|c| format!(" (container={c})")).unwrap_or_default());
        suggest_similar(&sym_t, &paths, name);
        return;
    }
    for &def in &defs {
        println!("{}", fmt_sym(&sym_t, &paths, def));
    }
    // BFS 全層から is_test=1 だけ拾う (depth = 起点からの呼び出し距離)。
    let mut tests: Vec<(u32, EntityId)> = Vec::new();
    for (i, layer) in caller_bfs(&call_t, &defs).iter().enumerate() {
        for &e in layer {
            if num(sym_t.entity(e).get("is_test")) == 1 {
                tests.push((i as u32 + 1, e));
            }
        }
    }
    if tests.is_empty() {
        println!("# {name} に届くテストなし (確定 edge のみ。候補 edge の見逃しは `callers {name}` の⚠で確認)");
        return;
    }
    tests.sort_by_key(|&(d, e)| {
        let er = sym_t.entity(e);
        (d, paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(), num(er.get("line")))
    });
    for (d, e) in tests.iter().take(limit) {
        println!("  [d{d}] {}", fmt_sym(&sym_t, &paths, *e));
    }
    if tests.len() > limit {
        println!("  … (+{} 件省略、--limit で全部)", tests.len() - limit);
    }
    println!("# {name} に届くテスト: {} 件 (確定 edge のみ = 下界)", tests.len());
    // そのまま貼れる実行ヒント (libtest は複数 filter を OR で受ける)。
    let names: Vec<String> = tests.iter().map(|&(_, e)| txt(sym_t.entity(e).get("name"))).collect::<HashSet<_>>().into_iter().collect();
    if names.len() <= TESTS_HINT_MAX {
        let mut ns = names;
        ns.sort();
        println!("# 実行: cargo test -- {}", ns.join(" "));
    }
}

/// `path <from> <to>` — from が to を(推移的に)呼ぶ最短経路を 1 本 (前方 BFS)。
pub fn cmd_path(args: &[String]) {
    let o = parse_opts(args);
    let (Some(from), Some(to)) = (o.pos.first(), o.pos.get(1)) else {
        eprintln!("usage: kenning path <from> <to> [--db P]");
        return;
    };
    run_path(&o.db, from, to);
}

fn run_path(db_path: &str, from: &str, to: &str) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    let srcs = defs_of(&sym_t, from, None);
    let tgts: HashSet<EntityId> = defs_of(&sym_t, to, None).into_iter().collect();
    if srcs.is_empty() || tgts.is_empty() {
        println!("# 定義が index に無い ({} / {})", if srcs.is_empty() { from } else { "ok" }, if tgts.is_empty() { to } else { "ok" });
        return;
    }
    // 多始点前方 BFS。parent で経路復元。始点が既に target ならそれ自身が経路。
    let mut parent: HashMap<EntityId, EntityId> = HashMap::new();
    let mut visited: HashSet<EntityId> = srcs.iter().copied().collect();
    let mut queue: std::collections::VecDeque<EntityId> = srcs.iter().copied().collect();
    let mut hit = srcs.iter().copied().find(|s| tgts.contains(s));
    while hit.is_none() {
        let Some(s) = queue.pop_front() else { break };
        for callee in direct_callees(&call_t, s) {
            if !visited.contains(&callee) {
                visited.insert(callee);
                parent.insert(callee, s);
                if tgts.contains(&callee) {
                    hit = Some(callee);
                    break;
                }
                queue.push_back(callee);
            }
        }
    }
    let Some(t) = hit else {
        println!("# {from} → {to}: 確定 edge で経路なし (候補/未解決 edge 経由なら在るかも)");
        return;
    };
    // t から parent を遡って経路復元 → 反転。
    let mut chain = vec![t];
    let mut cur = t;
    while let Some(&p) = parent.get(&cur) {
        chain.push(p);
        cur = p;
    }
    chain.reverse();
    for (i, &e) in chain.iter().enumerate() {
        let er = sym_t.entity(e);
        let path = paths.get(&ref_of(er.get("file"))).map(String::as_str).unwrap_or("?");
        println!("  {}{}\t{}:{}", if i == 0 { "" } else { "→ " }, sym_qual(&sym_t, e), path, num(er.get("line")));
    }
    println!("# {} hops (確定 edge のみ・最短)", chain.len() - 1);
}

/// sym eid → "Container::name" (経路の compact ラベル)。
fn sym_qual(sym_t: &Table, eid: EntityId) -> String {
    let er = sym_t.entity(eid);
    let ct = txt(er.get("container"));
    let nm = txt(er.get("name"));
    if ct.is_empty() { nm } else { format!("{ct}::{nm}") }
}

/// `outline <path>` — ファイルの symbol 一覧。Read せず構造を掴む (path は末尾一致でも可)。
pub fn cmd_outline(args: &[String]) {
    let o = parse_opts(args);
    let Some(path_arg) = o.pos.first() else {
        eprintln!("usage: kenning outline <path> [--db P]");
        return;
    };
    let Some(db) = open_ro(&o.db) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let paths = file_paths(&file_t);

    let fe = match file_t.where_eq("path", path_arg.as_str()).find_one().unwrap() {
        Some(e) => Some(e),
        None => paths.iter().find(|(_, p)| p.ends_with(path_arg.as_str())).map(|(e, _)| *e),
    };
    let Some(fe) = fe else {
        eprintln!("# file not found: {path_arg}");
        return;
    };
    let syms = sym_t.all().where_ref("file", fe).find().unwrap();
    println!("# {} : {} symbols", paths.get(&fe).map(String::as_str).unwrap_or("?"), syms.len());
    print_syms(&sym_t, &paths, &syms, o.limit, true);
}

/// `stats` — index の規模と名前解決率。
pub fn cmd_stats(args: &[String]) {
    let o = parse_opts(args);
    let Some(db) = open_ro(&o.db) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let n_call = call_t.all().count().unwrap();
    println!(
        "index: {} files / {} symbols / {} call-sites",
        file_t.all().count().unwrap(),
        sym_t.all().count().unwrap(),
        n_call
    );
    print!("resolve:");
    let mut resolved = 0usize;
    for (r, nm) in RES_NAMES.iter().enumerate() {
        let c = call_t.where_eq("res", r as u32).count().unwrap();
        if r == R_UNIQUE as usize || r == R_QUALIFIED as usize {
            resolved += c;
        }
        print!(" {nm}={c}");
    }
    println!(
        " → 解決率 {:.1}%",
        if n_call > 0 { resolved as f64 * 100.0 / n_call as f64 } else { 0.0 }
    );

    // crate 別の symbol 数 (新しい repo での当たり付け = どこにコードの重心があるか)。
    let mut set = std::collections::BTreeSet::new();
    for e in file_t.all().find().unwrap() {
        set.insert(txt(file_t.entity(e).get("crate_")));
    }
    let mut rows: Vec<(usize, String)> = set
        .into_iter()
        .map(|c| (sym_t.where_eq("crate_", c.as_str()).count().unwrap(), c))
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    println!("crates ({}):", rows.len());
    for (n, c) in rows.iter().take(20) {
        println!("  {n:>5}  {c}");
    }
    if rows.len() > 20 {
        println!("  … (+{} crates)", rows.len() - 20);
    }
}

// ─────────────────────────── bench (デモ / 計測) ───────────────────────────

/// best-of-N latency で count クエリを計測して 1 行出す。
fn timed<F: Fn() -> usize>(label: &str, f: F) -> usize {
    let mut best = Duration::MAX;
    let mut cnt = 0;
    for _ in 0..50 {
        let t = Instant::now();
        cnt = f();
        best = best.min(t.elapsed());
    }
    println!("  {:<52} = {:>7} 件  [{:?}]", label, cnt, best);
    cnt
}

fn txt(v: Option<Value>) -> String {
    match v {
        Some(Value::Text(s)) => s,
        _ => String::new(),
    }
}
fn num(v: Option<Value>) -> u32 {
    match v {
        Some(Value::Number(n)) => n as u32,
        _ => u32::MAX,
    }
}
fn ref_of(v: Option<Value>) -> EntityId {
    match v {
        Some(Value::Ref(e)) => e,
        _ => 0,
    }
}
fn kind_name(k: u32) -> &'static str {
    KIND_NAMES.get(k as usize).copied().unwrap_or("?")
}
fn vis_name(v: u32) -> &'static str {
    VIS_NAMES.get(v as usize).copied().unwrap_or("?")
}
/// facet 文字列 → コード。未知は None (呼び出し側で無視)。
fn kind_code(s: &str) -> Option<u32> {
    KIND_NAMES.iter().position(|k| *k == s).map(|i| i as u32)
}
fn vis_code(s: &str) -> Option<u32> {
    match s {
        "private" => Some(V_PRIV),
        _ => VIS_NAMES.iter().position(|k| *k == s).map(|i| i as u32),
    }
}
fn bool01(s: &str) -> u32 {
    matches!(s, "1" | "true" | "yes" | "y") as u32
}

// ═══════════════════ bench suite (再現可能な計測 — 逸話でなく分布を出す) ═══════════════════
//
// `bench [quality|agent|micro|all]` — 出力は markdown (RESULTS.md にリダイレクトする前提)。
// quality: ランダム N シンボルで grep 相当ヒット vs 確実/候補 callers → ノイズ除去率の分布。
// agent:   被呼数上位の固定質問を grep 経路 (rg 出力 + 文脈 Read のバイトモデル) vs
//          kenning 経路 (実バイナリ実行の実出力バイト) で再生 → token 削減比の分布。
// micro:   コマンド latency (warm)。
// 乱数は固定 seed の LCG (依存なし・決定的)。コーパス取得は bench/corpus.sh (tag 固定)。

/// 決定的な擬似乱数 (splitmix64)。ベンチのシンボル抽出専用。
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
}

/// index 済み全ファイルを disk から読む (path, src)。ベンチの grep 相当スキャンと Read モデルに使う。
fn bench_files(file_t: &Table) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = file_t
        .all()
        .find()
        .unwrap()
        .into_iter()
        .filter_map(|e| {
            let p = txt(file_t.entity(e).get("path"));
            std::fs::read_to_string(&p).ok().map(|src| (p, src))
        })
        .collect();
    v.sort();
    v
}

/// `\bname\s*\(` 相当のヒット行 (1-based) を返す — rg で「name の呼び出し」を探す時の定番 pattern。
/// (turbofish `name::<T>(` は rg 同様拾えない。モデルの限界として RESULTS に明記)
fn grep_call_hit_lines(src: &str, name: &str) -> Vec<usize> {
    let mut hits = Vec::new();
    let b = src.as_bytes();
    let nb = name.as_bytes();
    let mut i = 0;
    while let Some(p) = src[i..].find(name) {
        let j = i + p;
        let pre_ok = j == 0 || !(b[j - 1].is_ascii_alphanumeric() || b[j - 1] == b'_');
        let mut k = j + nb.len();
        while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
            k += 1;
        }
        if pre_ok && k < b.len() && b[k] == b'(' {
            hits.push(src[..j].bytes().filter(|&c| c == b'\n').count() + 1);
        }
        i = j + nb.len();
    }
    hits
}

fn median_u(mut v: Vec<usize>) -> usize {
    if v.is_empty() {
        return 0;
    }
    v.sort();
    v[v.len() / 2]
}

fn median_f(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// 呼ばれている fn/method の distinct 名 (決定的順序)。quality の母集団 & agent の質問源。
fn bench_target_names(sym_t: &Table, call_t: &Table) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for e in sym_t.all().find().unwrap() {
        let er = sym_t.entity(e);
        let k = num(er.get("kind"));
        if k != K_FN && k != K_METHOD {
            continue;
        }
        let name = txt(er.get("name"));
        if name.len() < 3 || !seen.insert(name.clone()) {
            continue;
        }
        if call_t.where_eq("callee", name.as_str()).count().unwrap() > 0 {
            names.push(name);
        }
    }
    names.sort();
    names
}

/// name の呼び出し分類: (確実=callee_sym が同名 def, 別symに確定, 候補=未解決の名前一致)。
fn classify_calls(sym_t: &Table, call_t: &Table, name: &str) -> (usize, usize, usize) {
    let defs: HashSet<EntityId> = sym_t.where_eq("name", name).find().unwrap().into_iter().collect();
    let (mut sure, mut other, mut cand) = (0, 0, 0);
    for c in call_t.where_eq("callee", name).find().unwrap() {
        match call_t.entity(c).get("callee_sym") {
            Some(Value::Ref(d)) => {
                if defs.contains(&d) {
                    sure += 1;
                } else {
                    other += 1;
                }
            }
            _ => cand += 1,
        }
    }
    (sure, other, cand)
}

/// ③品質: ランダム n シンボルで grep 相当ヒット vs kenning 分類を全数比較。
fn bench_quality(sym_t: &Table, call_t: &Table, files: &[(String, String)], n: usize, seed: u64) {
    let mut pool = bench_target_names(sym_t, call_t);
    let mut rng = Lcg(seed);
    // Fisher–Yates で先頭 n 件を決定的に抽出
    for i in 0..pool.len().saturating_sub(1).min(n) {
        let j = i + (rng.next() as usize) % (pool.len() - i);
        pool.swap(i, j);
    }
    let sample: Vec<String> = pool.into_iter().take(n).collect();

    println!("### quality — grep 相当ヒット vs 精密 callers (n={}, seed={})\n", sample.len(), seed);
    println!("grep 相当 = `\\bNAME\\s*\\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。");
    println!("確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。\n");

    let (mut greps, mut sures, mut boths, mut noise) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut rows: Vec<(usize, String, usize, usize, usize)> = Vec::new();
    for name in &sample {
        let g: usize = files.iter().map(|(_, s)| grep_call_hit_lines(s, name).len()).sum();
        let (sure, _other, cand) = classify_calls(sym_t, call_t, name);
        greps.push(g);
        sures.push(sure);
        boths.push(sure + cand);
        if g > 0 {
            noise.push((g.saturating_sub(sure + cand)) as f64 / g as f64);
        }
        rows.push((g, name.clone(), sure, cand, g.saturating_sub(sure + cand)));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    println!("| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |");
    println!("|---|---|---|---|---|");
    for (g, name, sure, cand, nz) in rows.iter().take(10) {
        println!("| {name} | {g} | {sure} | {cand} | {nz} |");
    }
    println!("| … | (上位 10 件のみ表示) | | | |\n");
    println!(
        "**中央値**: grep {} 行 → 確実 {} + 候補 {} = 検討対象 {} 行、ノイズ率 {:.0}%\n",
        median_u(greps),
        median_u(sures.clone()),
        median_u(boths.iter().zip(&sures).map(|(b, s)| b - s).collect()),
        median_u(boths),
        median_f(noise) * 100.0
    );
}

/// 「定義が一意」な被呼シンボルを確実 caller 数降順で (agent/beyond の質問選定、DRY)。
fn unique_def_ranked(sym_t: &Table, call_t: &Table) -> Vec<(usize, String)> {
    let mut ranked: Vec<(usize, String)> = bench_target_names(sym_t, call_t)
        .into_iter()
        .filter(|n| sym_t.where_eq("name", n.as_str()).count().unwrap() == 1)
        .map(|n| (classify_calls(sym_t, call_t, &n).0, n))
        .filter(|(s, _)| *s >= 3)
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    ranked
}

/// grep 経路の共通コストモデル (agent スイートと同一): rg 出力行 + ヒット各ファイル 40 行 Read。
/// hit_fn はファイル内容 → ヒット行番号 (1-based)。返り値 (bytes, tool 呼び出し数, ヒット数)。
fn grep_route_cost(files: &[(String, String)], hit_fn: &dyn Fn(&str) -> Vec<usize>) -> (usize, usize, usize) {
    let (mut bytes, mut nfiles, mut nhits) = (0usize, 0usize, 0usize);
    for (path, src) in files {
        let hits = hit_fn(src);
        if hits.is_empty() {
            continue;
        }
        nfiles += 1;
        nhits += hits.len();
        let lines: Vec<&str> = src.lines().collect();
        for &l in &hits {
            bytes += path.len() + 8 + lines.get(l - 1).map_or(0, |s| s.len()) + 1;
        }
        let l = hits[0];
        let (a, b) = (l.saturating_sub(20), (l + 20).min(lines.len()));
        bytes += lines[a..b].iter().map(|s| s.len() + 1).sum::<usize>();
    }
    (bytes, 1 + nfiles, nhits)
}

/// 行内に word が識別子境界付きで現れるか。
fn word_in(line: &str, word: &str) -> bool {
    let b = line.as_bytes();
    let mut i = 0;
    while let Some(p) = line[i..].find(word) {
        let j = i + p;
        let pre = j == 0 || !(b[j - 1].is_ascii_alphanumeric() || b[j - 1] == b'_');
        let k = j + word.len();
        let post = k >= b.len() || !(b[k].is_ascii_alphanumeric() || b[k] == b'_');
        if pre && post {
            return true;
        }
        i = j + word.len();
    }
    false
}

/// 条件付き行ヒット (grep モデル用の汎用スキャナ)。
fn line_hits(src: &str, pred: &dyn Fn(&str) -> bool) -> Vec<usize> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| pred(l))
        .map(|(i, _)| i + 1)
        .collect()
}

/// kenning を別プロセスで実行して stdout バイト数を返す (実測、鮮度チェック抜き)。
fn cs_run_bytes(exe: &std::path::Path, db_path: &str, args: &[&str]) -> usize {
    let mut all: Vec<&str> = args.to_vec();
    all.extend_from_slice(&["--db", db_path]);
    std::process::Command::new(exe)
        .args(&all)
        .env("KENNING_NO_STALE", "1")
        .output()
        .map(|o| o.stdout.len())
        .unwrap_or(0)
        .max(1)
}

/// beyond-search: サーチ以外のクエリ (impact / impls / outline / def / faceted) を
/// agent スイートと同じ楽観 grep モデル vs 実出力で比較。「grep に原理的に不可能」系の
/// 主張を逸話でなく分布にする。
fn bench_beyond(db_path: &str, db: &Database, sym_t: &Table, call_t: &Table, files: &[(String, String)]) {
    let exe = std::env::current_exe().unwrap();
    println!("### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)\n");
    println!("grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに");
    println!("grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。\n");

    let ranked = unique_def_ranked(sym_t, call_t);

    // ── impact: 推移的 callers (変えると壊れる範囲) ──
    let mut rows: Vec<(String, usize, usize, usize, usize, f64)> = Vec::new();
    for (_, name) in ranked.iter().take(5) {
        // 実 BFS で訪問シンボル集合を得る (grep 手動 BFS が辿るのと同じ集合)
        let defs = defs_of(sym_t, name, None);
        let mut visited: HashSet<EntityId> = defs.iter().copied().collect();
        let mut frontier = defs;
        let mut depth = 0;
        while !frontier.is_empty() && depth < 64 {
            let mut next = Vec::new();
            for &s in &frontier {
                for c in direct_callers(call_t, s) {
                    if c != 0 && visited.insert(c) {
                        next.push(c);
                    }
                }
            }
            frontier = next;
            depth += 1;
        }
        let names: HashSet<String> = visited.iter().map(|&s| txt(sym_t.entity(s).get("name"))).collect();
        let (mut g_bytes, mut g_calls) = (0usize, 0usize);
        for n in &names {
            let (b, c, _) = grep_route_cost(files, &|src| grep_call_hit_lines(src, n));
            g_bytes += b;
            g_calls += c;
        }
        let cs = cs_run_bytes(&exe, db_path, &["impact", name]);
        rows.push((name.clone(), visited.len(), g_bytes, g_calls, cs, g_bytes as f64 / cs as f64));
    }
    if !rows.is_empty() {
        println!("**impact** (推移的 callers、上位 5 問):\n");
        println!("| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |");
        println!("|---|---|---|---|---|---|");
        for (n, v, gb, gc, cb, r) in &rows {
            println!("| impact {n} | {v} | {gb} | {gc} | {cb} | {r:.0}x |");
        }
        println!(
            "\n中央値: **{:.0}x**、tool 呼び出し {} 回 → 1 回\n",
            median_f(rows.iter().map(|r| r.5).collect()),
            median_u(rows.iter().map(|r| r.3).collect())
        );
    }

    // ── impls: go-to-implementation ──
    if let Some(impl_t) = db.get_table("impl") {
        let mut freq: HashMap<String, usize> = HashMap::new();
        for e in impl_t.all().find().unwrap() {
            *freq.entry(txt(impl_t.entity(e).get("trait_name"))).or_default() += 1;
        }
        let mut traits: Vec<(usize, String)> = freq.into_iter().filter(|(t, _)| !t.is_empty()).map(|(t, n)| (n, t)).collect();
        traits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut rows: Vec<(String, usize, usize, usize, f64)> = Vec::new();
        for (n_impls, t) in traits.iter().take(5).filter(|(n, _)| *n >= 2) {
            let (gb, gc, _) = grep_route_cost(files, &|src| {
                line_hits(src, &|l| l.contains("impl") && word_in(l, t))
            });
            let cs = cs_run_bytes(&exe, db_path, &["impls", t]);
            rows.push((t.clone(), *n_impls, gb, gc, gb as f64 / cs as f64));
        }
        if !rows.is_empty() {
            println!("**impls** (trait→実装型、impl 数上位):\n");
            println!("| question | impls | grep bytes | grep calls | 圧縮比 |");
            println!("|---|---|---|---|---|");
            for (t, n, gb, gc, r) in &rows {
                println!("| impls {t} | {n} | {gb} | {gc} | {r:.1}x |");
            }
            println!("\n中央値: **{:.1}x**\n", median_f(rows.iter().map(|r| r.4).collect()));
        }
    }

    // ── outline: ファイル構造 (Read 全文の代替) ──
    let mut biggest: Vec<&(String, String)> = files.iter().collect();
    biggest.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    let mut rows: Vec<(String, usize, usize, f64)> = Vec::new();
    for (path, src) in biggest.iter().take(5) {
        let cs = cs_run_bytes(&exe, db_path, &["outline", path]);
        rows.push((path.clone(), src.len(), cs, src.len() as f64 / cs as f64));
    }
    println!("**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):\n");
    println!("| file | Read bytes | outline bytes | 圧縮比 |");
    println!("|---|---|---|---|");
    for (p, fb, ob, r) in &rows {
        let short = p.rsplit('/').next().unwrap_or(p);
        println!("| {short} | {fb} | {ob} | {r:.0}x |");
    }
    println!("\n中央値: **{:.0}x**\n", median_f(rows.iter().map(|r| r.3).collect()));

    // ── def: hover 相当 (定義位置 + sig + doc) ──
    let mut rows: Vec<(String, usize, usize, usize, f64)> = Vec::new();
    for (_, name) in ranked.iter().take(10) {
        let kws = ["fn ", "struct ", "enum ", "trait ", "const "];
        let (gb, gc, _) = grep_route_cost(files, &|src| {
            line_hits(src, &|l| word_in(l, name) && kws.iter().any(|k| l.contains(k)))
        });
        let cs = cs_run_bytes(&exe, db_path, &["def", name]);
        rows.push((name.clone(), gb, gc, cs, gb as f64 / cs as f64));
    }
    if !rows.is_empty() {
        println!("**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg \"fn NAME\"` + 前後 Read):\n");
        println!(
            "中央値: grep {} B / {} 回 → def {} B / 1 回 = **{:.1}x**\n",
            median_u(rows.iter().map(|r| r.1).collect()),
            median_u(rows.iter().map(|r| r.2).collect()),
            median_u(rows.iter().map(|r| r.3).collect()),
            median_f(rows.iter().map(|r| r.4).collect())
        );
    }

    // ── faceted: grep で表現不能 (比較対象なし、latency のみ) ──
    let t = Instant::now();
    let n = sym_t
        .where_eq("kind", K_METHOD)
        .where_eq("vis", V_PUB)
        .where_eq("is_test", 0u32)
        .count()
        .unwrap();
    println!(
        "**faceted** (`kind:method vis:pub test:0`): {} 件 {:?} — grep では表現不能 (比較なし、能力差)\n",
        n,
        t.elapsed()
    );
}

/// ④エージェント級: 「誰が呼ぶ？」固定質問を grep 経路 vs kenning 経路で再生し、
/// tool 出力バイト数 (≈token) と呼び出し回数を比較。
/// grep 経路モデル: rg 出力 (`path:line:linetext`) + ヒットのある各ファイルを 40 行 Read
/// (ファイルごと 1 回 = 楽観的な agent。実際は再 grep や全 Read も多いので保守的な下限)。
fn bench_agent(db_path: &str, root: &str, sym_t: &Table, call_t: &Table, files: &[(String, String)], nq: usize) {
    // 質問 = 「定義が一意」なシンボルの確実 caller 数上位 nq。一意に限るのは公平性のため:
    // `new` みたいな多重定義名は実際の agent も `Type::new` で grep するので、`\bname\(` モデルが
    // grep 側に不当に不利になる。一意名なら `\bname\(` はまさに agent が打つ手 = 対等な比較。
    let questions: Vec<String> = unique_def_ranked(sym_t, call_t).into_iter().take(nq).map(|(_, n)| n).collect();

    println!("### agent — 「誰が呼ぶ?」{} 問の tool 出力バイト比較\n", questions.len());
    println!("質問 = 定義が一意な被呼上位シンボル (grep 側も `\\bname\\(` で正確に狙える公平条件)。");
    println!("grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。");
    println!("kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。\n");
    println!("| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |");
    println!("|---|---|---|---|---|---|");

    let exe = std::env::current_exe().unwrap();
    // 対競合 (wall-clock): rg / ast-grep が入っていれば同じ問いを同条件で測る (無ければ skip)
    let has_rg = std::process::Command::new("rg").arg("--version").output().is_ok();
    let has_sg = std::process::Command::new("ast-grep").arg("--version").output().is_ok();
    let (mut rg_ms, mut cs_ms) = (Vec::new(), Vec::new());
    let (mut ratios, mut gcalls) = (Vec::new(), Vec::new());
    // ast-grep 行: (question, hits, bytes, ms, 確実, 候補, cs_bytes)
    let mut sg_rows: Vec<(String, usize, usize, f64, usize, usize, usize)> = Vec::new();
    for name in &questions {
        let (mut rg_bytes, mut read_bytes) = (0usize, 0usize);
        let mut nfiles = 0usize;
        for (path, src) in files {
            let hits = grep_call_hit_lines(src, name);
            if hits.is_empty() {
                continue;
            }
            nfiles += 1;
            let lines: Vec<&str> = src.lines().collect();
            for &l in &hits {
                rg_bytes += path.len() + 2 + 6 + lines.get(l - 1).map_or(0, |s| s.len()) + 1;
            }
            let l = hits[0];
            let (a, b) = (l.saturating_sub(20), (l + 20).min(lines.len()));
            read_bytes += lines[a..b].iter().map(|s| s.len() + 1).sum::<usize>();
        }
        let grep_total = rg_bytes + read_bytes;
        if has_rg {
            let t = Instant::now();
            let _ = std::process::Command::new("rg")
                .args(["-n", &format!("\\b{name}\\s*\\("), root])
                .output();
            rg_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        let t = Instant::now();
        let out = std::process::Command::new(&exe)
            .args(["callers", name, "--db", db_path])
            .env("KENNING_NO_STALE", "1")
            .output()
            .expect("self exec");
        cs_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        let cs_bytes = out.stdout.len().max(1);
        if has_sg {
            // 呼び出しの 3 形 (裸/メソッド/修飾) を列挙 — ast-grep はこの列挙をユーザーが背負う
            let pats = [
                format!("{name}($$$)"),
                format!("$R.{name}($$$)"),
                format!("$P::{name}($$$)"),
            ];
            let (mut hits, mut bytes, mut ms) = (0usize, 0usize, 0f64);
            for p in &pats {
                let t = Instant::now();
                if let Ok(o) = std::process::Command::new("ast-grep")
                    .args(["run", "--pattern", p, "--lang", "rust", root])
                    .output()
                {
                    ms += t.elapsed().as_secs_f64() * 1000.0;
                    bytes += o.stdout.len();
                }
                // 件数は json で数える (計時外 — 表示バイトの二重測定を避ける)
                if let Ok(o) = std::process::Command::new("ast-grep")
                    .args(["run", "--pattern", p, "--lang", "rust", root, "--json=compact"])
                    .output()
                {
                    hits += String::from_utf8_lossy(&o.stdout).matches("\"file\":").count();
                }
            }
            let (sure, _other, cand) = classify_calls(sym_t, call_t, name);
            sg_rows.push((name.clone(), hits, bytes, ms, sure, cand, cs_bytes));
        }
        let ratio = grep_total as f64 / cs_bytes as f64;
        ratios.push(ratio);
        gcalls.push(1 + nfiles);
        println!(
            "| callers {name} | {grep_total} | {} | {cs_bytes} | 1 | {ratio:.1}x |",
            1 + nfiles
        );
    }
    println!(
        "\n**中央値**: 圧縮比 **{:.1}x**、grep 経路の tool 呼び出し {} 回 → 1 回\n",
        median_f(ratios),
        median_u(gcalls)
    );
    if has_rg {
        println!(
            "**単発 wall-clock 中央値**: rg {:.1}ms vs kenning {:.1}ms (両者プロセス起動込み。\nkenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)\n",
            median_f(rg_ms),
            median_f(cs_ms.clone())
        );
    }
    if has_sg && !sg_rows.is_empty() {
        println!("### agent — vs ast-grep (構造検索アプリ、同じ質問)\n");
        println!("ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。");
        println!("ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、");
        println!("**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは");
        println!("答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。\n");
        println!("| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |");
        println!("|---|---|---|---|---|---|---|");
        let n = sg_rows.len().min(cs_ms.len());
        for (i, (q, hits, bytes, ms, sure, cand, csb)) in sg_rows.iter().take(n).enumerate() {
            println!(
                "| callers {q} | {hits} | {bytes} | {ms:.0} | {sure}+{cand} | {csb} | {:.1} |",
                cs_ms[i]
            );
        }
        println!(
            "\n**中央値**: ast-grep {:.0}ms / {}B vs kenning {:.1}ms / {}B — 構造一致としては同数を拾うが、\n「どの定義か」の確定・impact/path/faceted は ast-grep には無い\n",
            median_f(sg_rows.iter().map(|r| r.3).collect()),
            median_u(sg_rows.iter().map(|r| r.2).collect()),
            median_f(cs_ms),
            median_u(sg_rows.iter().map(|r| r.6).collect())
        );
    }
}

/// ①マイクロ: コマンドの warm latency (generic — どの repo の db でも動く)。
fn bench_micro(db_path: &str, sym_t: &Table, call_t: &Table, files: &[(String, String)]) {
    println!("### micro — warm latency\n```");
    let t = Instant::now();
    let db2 = Database::open_readonly(db_path).unwrap();
    println!("open(readonly): {:?}", t.elapsed());
    drop(db2);

    let n_sym = sym_t.all().count().unwrap();
    let n_call = call_t.all().count().unwrap();
    println!("index: {} files / {} symbols / {} call-sites", files.len(), n_sym, n_call);

    timed("kind=fn", || sym_t.where_eq("kind", K_FN).count().unwrap());
    timed("pub fn", || {
        sym_t.where_eq("kind", K_FN).where_eq("vis", V_PUB).count().unwrap()
    });
    timed("pub async fn 非test", || {
        sym_t.where_eq("kind", K_FN).where_eq("vis", V_PUB)
            .where_eq("is_async", 1u32).where_eq("is_test", 0u32).count().unwrap()
    });
    // 点クエリは被呼最多シンボルで (どの repo でも存在する)
    if let Some(name) = bench_target_names(sym_t, call_t)
        .into_iter()
        .max_by_key(|n| call_t.where_eq("callee", n.as_str()).count().unwrap())
    {
        timed(&format!("def {name}"), || sym_t.where_eq("name", name.as_str()).count().unwrap());
        timed(&format!("callers {name} (名前一致)"), || {
            call_t.where_eq("callee", name.as_str()).count().unwrap()
        });
        if let Some(d) = sym_t.where_eq("name", name.as_str()).find().unwrap().first().copied() {
            timed(&format!("callers {name} (確実 逆引き)"), || {
                call_t.where_eq("callee_sym", Value::Ref(d)).count().unwrap()
            });
        }
    }
    println!("```\n");
}

/// `bench [quality|agent|micro|all] [--db P] [--n N] [--seed S]` — markdown を stdout へ。
pub fn cmd_bench(args: &[String]) {
    let mut sub = "all".to_string();
    let mut n = 100usize;
    let mut nq = 20usize;
    let mut seed = 42u64;
    let mut rest: Vec<String> = Vec::new();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "quality" | "agent" | "beyond" | "micro" | "all" => sub = a.clone(),
            "--n" => n = it.next().and_then(|v| v.parse().ok()).unwrap_or(n),
            "--nq" => nq = it.next().and_then(|v| v.parse().ok()).unwrap_or(nq),
            "--seed" => seed = it.next().and_then(|v| v.parse().ok()).unwrap_or(seed),
            _ => rest.push(a.clone()),
        }
    }
    let o = parse_opts(&rest); // --db / auto-derive / auto-update はいつもの経路に乗せる
    let Some(db) = open_ro(&o.db) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let files = bench_files(&file_t);

    // ヘッダ: どの repo・どの精度の index かを自己記述 (結果の再現に必須)
    let (root, _) = read_meta(&db).unwrap_or_default();
    let baked = db
        .get_table("meta")
        .and_then(|t| t.all().find().unwrap().into_iter().next().map(|e| matches!(t.entity(e).get("baked_at"), Some(Value::Number(b)) if b > 0)))
        .unwrap_or(false);
    let resolved: usize = [R_UNIQUE, R_QUALIFIED]
        .iter()
        .map(|&r| call_t.where_eq("res", r).count().unwrap())
        .sum();
    let n_call = call_t.all().count().unwrap();
    println!(
        "corpus `{}` — {} files / {} call-sites / 解決率 {:.1}% / {}\n",
        root,
        files.len(),
        n_call,
        if n_call > 0 { resolved as f64 * 100.0 / n_call as f64 } else { 0.0 },
        if baked { "**baked (SCIP)**" } else { "syn-only (未 bake)" }
    );

    if sub == "quality" || sub == "all" {
        bench_quality(&sym_t, &call_t, &files, n, seed);
    }
    if sub == "agent" || sub == "all" {
        bench_agent(&o.db, &root, &sym_t, &call_t, &files, nq);
    }
    if sub == "beyond" || sub == "all" {
        bench_beyond(&o.db, &db, &sym_t, &call_t, &files);
    }
    if sub == "micro" || sub == "all" {
        bench_micro(&o.db, &sym_t, &call_t, &files);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bake_lock_reclaims_dead_pid_but_respects_live() {
        let d = tmp_tree("bakelock");
        // 死んだ pid の残骸 → 自動回収して取得成功、Drop で解放
        std::fs::write(d.join("bake.lock"), "999999999").unwrap();
        let l = BakeLock::acquire(&d);
        assert!(l.is_some(), "dead pid の stale lock は回収されるべき");
        drop(l);
        assert!(!d.join("bake.lock").exists(), "Drop で lock 解放");
        // 書き込み途中で死んだ空 lock も残骸扱い
        std::fs::write(d.join("bake.lock"), "").unwrap();
        assert!(BakeLock::acquire(&d).is_some());
        // 生きてる pid (自プロセス) の lock は尊重して取得失敗
        std::fs::write(d.join("bake.lock"), std::process::id().to_string()).unwrap();
        assert!(BakeLock::acquire(&d).is_none(), "live な lock は奪わない");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn end_line_of_covers_body_and_attrs() {
        let f: syn::ItemFn = syn::parse_str("fn f() {\n    let x = 1;\n    x;\n}").unwrap();
        assert_eq!(line_of(f.sig.ident.span()), 1);
        assert_eq!(end_line_of(&f), 4); // 閉じ brace の行まで
        let s: syn::ItemStruct = syn::parse_str("#[derive(Debug)]\nstruct S {\n    a: u32,\n}").unwrap();
        assert_eq!(end_line_of(&s), 4);
        let c: syn::ItemConst = syn::parse_str("const X: u32 = 1;").unwrap();
        assert_eq!(end_line_of(&c), 1); // 単一行 item は start == end
    }

    #[test]
    fn extend_up_grabs_docs_and_attrs_only() {
        let lines = vec!["use x;", "", "/// doc1", "/// doc2", "#[inline]", "fn f() {}"];
        assert_eq!(extend_up(&lines, 6), 3); // fn(6行目) → /// doc1(3行目) まで拡張、空行(2)で停止
        assert_eq!(extend_up(&lines, 1), 1); // 先頭はそのまま
        let plain = vec!["fn a() {}", "fn b() {}"];
        assert_eq!(extend_up(&plain, 2), 2); // 直上がコードなら拡張しない
    }

    #[test]
    fn trim_src_caps_long_lines() {
        assert_eq!(trim_src("  let x = 1;  "), "let x = 1;");
        let long = "x".repeat(SRC_LINE_MAX + 30);
        let t = trim_src(&long);
        assert_eq!(t.chars().count(), SRC_LINE_MAX + 1); // cap + '…'
        assert!(t.ends_with('…'));
    }

    fn tmp_tree(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kenning-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn pkg_name_of_reads_package_name() {
        let d = tmp_tree("pkg");
        let ct = d.join("Cargo.toml");
        std::fs::write(&ct, "[package]\nname = \"foo-bar\"\nversion = \"0.1.0\"\n").unwrap();
        assert_eq!(pkg_name_of(&ct), Some("foo-bar".to_string()));
        // virtual workspace manifest ([package] 無し) は None
        std::fs::write(&ct, "[workspace]\nmembers = [\"a\"]\n").unwrap();
        assert_eq!(pkg_name_of(&ct), None);
        // 他 section の name は拾わない
        std::fs::write(&ct, "[dependencies]\nname = \"wrong\"\n\n[package]\nversion = \"1\"\nname = \"right\"\n").unwrap();
        assert_eq!(pkg_name_of(&ct), Some("right".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn crate_of_nearest_ancestor_cargo_toml() {
        let d = tmp_tree("crateof");
        // root/Cargo.toml (root-pkg) + root/sub/Cargo.toml (sub-pkg) — root+サブ crate 混在レイアウト
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("sub/src")).unwrap();
        std::fs::write(d.join("Cargo.toml"), "[package]\nname = \"root-pkg\"\n").unwrap();
        std::fs::write(d.join("sub/Cargo.toml"), "[package]\nname = \"sub-pkg\"\n").unwrap();
        let mut cache = HashMap::new();
        let f_root = d.join("src/a.rs");
        let f_sub = d.join("sub/src/b.rs");
        assert_eq!(crate_of(&f_root.to_string_lossy(), &mut cache), "root-pkg");
        assert_eq!(crate_of(&f_sub.to_string_lossy(), &mut cache), "sub-pkg");
        // cache が dir 単位で効く (同 dir の別ファイルは同じ答え)
        let f_sub2 = d.join("sub/src/c.rs");
        assert_eq!(crate_of(&f_sub2.to_string_lossy(), &mut cache), "sub-pkg");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rust_files_prunes_target_and_hidden_dirs() {
        let d = tmp_tree("walk");
        for sub in ["src", "target/debug", ".store/blobs", ".git", "sub"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        for f in ["src/a.rs", "target/debug/gen.rs", ".store/blobs/snap.rs", ".git/hook.rs", "sub/b.rs", "src/.hidden.rs"] {
            std::fs::write(d.join(f), "").unwrap();
        }
        let mut got: Vec<String> = rust_files(&d.to_string_lossy())
            .map(|p| p.strip_prefix(&d).unwrap().to_string_lossy().into_owned())
            .collect();
        got.sort();
        assert_eq!(got, ["src/a.rs", "sub/b.rs"]);
        // root 自身が隠し名でも depth 0 は素通し (直指定で index できる)
        let hidden_root = d.join(".store/blobs");
        assert_eq!(rust_files(&hidden_root.to_string_lossy()).count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn grep_call_hits_word_boundary_and_paren() {
        let src = "foo();\nbar.foo ();\nxfoo();\nfoo_bar();\n// foo(\n\"foo(\"\nfoo\n";
        // 行 1 (foo()), 行 2 (.foo ()), 行 5 (コメント内も rg は拾う), 行 6 (文字列内も拾う)
        assert_eq!(grep_call_hit_lines(src, "foo"), vec![1, 2, 5, 6]);
        // 前方が識別子文字なら不一致 (xfoo)、後続が '(' でなければ不一致 (foo_bar は別 ident、素の foo)
        assert_eq!(grep_call_hit_lines("foo", "foo"), Vec::<usize>::new());
    }

    #[test]
    fn bench_medians_and_lcg_deterministic() {
        assert_eq!(median_u(vec![5, 1, 3]), 3);
        assert_eq!(median_u(vec![]), 0);
        assert!((median_f(vec![2.0, 1.0, 4.0]) - 2.0).abs() < 1e-9);
        let (mut a, mut b) = (Lcg(42), Lcg(42));
        assert_eq!((a.next(), a.next()), (b.next(), b.next())); // 同 seed → 同列 (再現性)
    }
}
