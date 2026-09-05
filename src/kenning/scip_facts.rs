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
        for doc in &idx.documents {
            let rel = if prefix.is_empty() { doc.relative_path.clone() } else { format!("{prefix}/{}", doc.relative_path) };
            doc_paths.insert(rel.clone());
            // doc の元ソースを 1 度だけ読み、行→char 列変換に使う(読めなければ byte 列のまま)。
            let src_lines: Option<Vec<String>> = std::fs::read_to_string(format!("{}/{}", scip_root, doc.relative_path))
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
        Scip { occ, pos2idx, doc_paths }
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
