//! rust-analyzer の SCIP を読む (bake で焼いた精密 facts の入口)。

use super::*;

// ─────────────────────────── SCIP (rust-analyzer の正確 facts) ───────────────────────────

/// 1 occurrence (参照点)。line/col は SCIP 準拠で 0-indexed。
pub(crate) struct ScipOcc {
    pub(crate) rel_path: String,
    pub(crate) line0: u32,
    pub(crate) col0: u32,
    pub(crate) symbol: String,
    pub(crate) roles: i32, // bit1=Definition, 2=Import, 4=WriteAccess, 8=ReadAccess
}

/// SCIP index を読み、全 occurrence を保持 + 位置 → occurrence index の map を作る。
/// 位置 join の鍵は (relative_path, line0, col0)。
pub(crate) struct Scip {
    pub(crate) occ: Vec<ScipOcc>,
    pos2idx: HashMap<(String, u32, u32), usize>,
    doc_paths: HashSet<String>, // SCIP が解析した doc の rel_path 集合 (no-occ 診断用)
    /// bake 後に内容が変わったので捨てた doc の数 (= その file は syn 層で解決する)。
    pub(crate) stale_docs: u32,
    /// bake 時の内容の記録 (`<scip>.src`) が無かった = どの doc も位置を保証できない (旧版の bake)。
    pub(crate) unverified: bool,
    /// 捨てた doc (bake 後に変わった file) で**定義**された symbol。位置は使えないが「repo の中の定義」なのは
    /// 確か — これを知らないと、変わっていない file からの呼び出しが「SCIP は知っているが index に無い = 外」
    /// に落ちる (定義側の file を編集した後の full 再 index で `defs_of` の caller 3 件が [external] になった)。
    pub(crate) stale_defs: HashSet<String>,
}

/// bake 時点の各 .rs の内容 fingerprint の置き場 (`<scip>.src`、行 = `絶対 path<TAB>hash`)。
pub(crate) fn scip_src_path(scip: &str) -> String {
    format!("{scip}.src")
}

/// bake を始める時点の内容を記録する。SCIP の答えは (file, 行, 列) で join するので、bake 後に
/// 行がずれた file に使うと**別の呼び出し・別の定義に確定する** (実例: 古い .scip の再利用で
/// `depth(..)` が `insert_call` の確実 caller になった)。記録と一致する file だけ SCIP を使う。
/// `out` に書く (bake は一時 file に書き、RA が成功してから .scip と一緒に置き換える — 焼いている途中に
/// 「新しい記録 + 古い .scip」の組が読まれると、ずれた答えが有効と判定される。実際に起きた)。
pub(crate) fn write_scip_src(out: &str, root: &str) {
    let mut s = String::new();
    for p in rust_files(root) {
        if let Ok(src) = std::fs::read_to_string(&p) {
            s += &format!("{}\t{}\n", canon(&p.to_string_lossy()), hash_u32(&src));
        }
    }
    let _ = std::fs::write(out, s);
}

/// 比較用の正規形 (`/tmp` ↔ `/private/tmp` のような symlink 差で別物にならないように)。
fn canon(p: &str) -> String {
    std::fs::canonicalize(p).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string())
}

fn read_scip_src(scip: &str) -> Option<HashMap<String, u32>> {
    let body = std::fs::read_to_string(scip_src_path(scip)).ok()?;
    Some(body.lines().filter_map(|l| l.split_once('\t')).filter_map(|(p, h)| Some((p.to_string(), h.parse().ok()?))).collect())
}
/// SCIP の `metadata.project_root` (file:// URI) を絶対パスに。無ければ None。
pub(crate) fn scip_project_root(idx: &scip::types::Index) -> Option<String> {
    let p = idx.metadata.as_ref()?.project_root.trim_start_matches("file://").trim_end_matches('/').to_string();
    if p.is_empty() { None } else { Some(p) }
}

impl Scip {
    /// `root` = index 対象ディレクトリ。SCIP のバイト列を char 列に直すのに元ソースを読む。
    pub(crate) fn load(path: &str, root: &str) -> Scip {
        use protobuf::Message;
        let bytes = std::fs::read(path).expect("read .scip");
        let idx = scip::types::Index::parse_from_bytes(&bytes).expect("decode .scip");
        let root = root.trim_end_matches('/');
        // SCIP の relative_path は **SCIP 自身の project_root** 基準。bake が repo root ではなく
        // その下の cargo workspace を焼いた場合 (#4)、index root とは原点がずれるので、その差分を
        // 鍵に足して join を合わせる。原点が一致する従来の .scip では prefix = "" (無変化)。
        let scip_root = scip_project_root(&idx).unwrap_or_else(|| root.to_string());
        let prefix = if scip_root == root { String::new() } else { rel_of(&scip_root, root) };
        if prefix.starts_with('/') {
            eprintln!("# ⚠ .scip の project_root ({scip_root}) が index root ({root}) の外 → 精密 facts は join できない");
        }
        let mut occ = Vec::new();
        let mut pos2idx = HashMap::new();
        let mut doc_paths = HashSet::new();
        let recorded = read_scip_src(path);
        let mut stale_docs = 0u32;
        let mut stale_defs = HashSet::new();
        for doc in &idx.documents {
            let rel = if prefix.is_empty() { doc.relative_path.clone() } else { format!("{prefix}/{}", doc.relative_path) };
            // doc の元ソースを 1 度だけ読み、行→char 列変換に使う。bake 時の内容と違えば doc ごと捨てる
            // (位置がずれた答えは誤確定の元)。記録が無い旧版の bake は 1 つも保証できないので全部捨てる。
            let abs = format!("{}/{}", scip_root, doc.relative_path);
            let src = std::fs::read_to_string(&abs).ok();
            let fresh = matches!((&recorded, &src), (Some(m), Some(s)) if m.get(&canon(&abs)) == Some(&hash_u32(s)));
            if !fresh {
                stale_docs += 1;
                stale_defs.extend(
                    doc.occurrences.iter().filter(|o| o.symbol_roles & 1 != 0 && !o.symbol.starts_with("local ")).map(|o| o.symbol.clone()),
                );
                continue;
            }
            doc_paths.insert(rel.clone());
            let src_lines: Option<Vec<String>> = src.map(|s| s.lines().map(str::to_string).collect());
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
                    pos2idx.insert((rel.clone(), line0, col0), occ.len());
                    occ.push(ScipOcc {
                        rel_path: rel.clone(),
                        line0,
                        col0,
                        symbol: o.symbol.clone(),
                        roles: o.symbol_roles,
                    });
                }
            }
        }
        if stale_docs > 0 {
            eprintln!(
                "# .scip の {stale_docs} file は{} → その分は syn 層で解決 (位置がずれた答えは使わない)",
                if recorded.is_none() { "内容の記録が無い旧版の bake" } else { " bake 後に変わった" }
            );
        }
        Scip { occ, pos2idx, doc_paths, stale_docs, unverified: recorded.is_none(), stale_defs }
    }
    /// syn の (rel_path, line 1-indexed, col 0-indexed) を SCIP 鍵に変換して symbol を引く。
    pub(crate) fn symbol_at(&self, rel_path: &str, line1: u32, col0: u32) -> Option<&str> {
        self.pos2idx
            .get(&(rel_path.to_string(), line1.saturating_sub(1), col0))
            .map(|&i| self.occ[i].symbol.as_str())
    }
    /// その rel_path を SCIP が解析したか (no-occ が「doc 欠落」か「位置ズレ」かの切り分け)。
    pub(crate) fn has_doc(&self, rel_path: &str) -> bool {
        self.doc_paths.contains(rel_path)
    }
}
