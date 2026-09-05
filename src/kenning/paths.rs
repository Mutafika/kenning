//! repo root / cache dir / db path / crate 名の解決。

use super::*;

/// ファイル内容の 32bit fingerprint (FNV-1a)。増分 index の変更検知用。
/// number 列は u32::MAX を sentinel に使うので、その値だけ避ける。
pub(crate) fn hash_u32(s: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    if h == u32::MAX { h ^ 1 } else { h }
}

/// repo root を推定: 最寄りの `.git` を持つ祖先、無ければ最上位の `Cargo.toml` を持つ祖先。
/// どちらも無ければ None (= 誤爆防止で自動 index しない)。儀式ゼロ (P-A) の要。
pub(crate) fn repo_root_of(dir: &str) -> Option<std::path::PathBuf> {
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

/// dir を絶対パスへ正規化 (解決できなければそのまま)。index/update の入口で必ず通す —
/// 相対のまま焼くと file.path が `./src/…` になり、別 cwd から叩いた時に出力の `path:line` を
/// そのまま Read へ渡せなくなる (#13 の別件)。meta.root も同じ理由で絶対に保つ。
pub(crate) fn abs_dir(dir: &str) -> String {
    std::fs::canonicalize(dir).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| dir.to_string())
}

/// 自動導出 db の置き場 (`~/.cache/kenning`)。作らない (無ければ「index が無い」と同義)。
pub(crate) fn cache_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(Path::new(&home).join(".cache/kenning"))
}

/// path が enchudb の db か。**v10 で db が単一ファイルから directory になった**ので、
/// 「directory なら repo」という素朴な判別が成立しなくなった — `update <dir> <db>` の位置引数を
/// is_dir だけで振り分けると db を repo と誤認し、**指定した db は古いまま cache に別 index が
/// 生える**(静かな取りこぼし。tests/core.rs の incremental_update_matches_full_reindex が gate)。
///
/// 判定は `Engine::probe` (mmap も lock も取らず stat のみ) で行う。内部の file 名に依存しない
/// ため。**`Incomplete` を db 側に入れてはいけない** — 素の repo directory は `header.seg` が
/// 無いので `Incomplete` を返す。「db になりきっていない directory」と「そもそも db でない
/// directory」は probe では区別できないので、repo 側に倒すのが安全 (index 対象として扱われる
/// だけで、db として開こうとして壊すことはない)。
pub fn is_db_path(p: &str) -> bool {
    matches!(Engine::probe(p), DbState::Ready | DbState::Damaged(_) | DbState::SingleFileLegacy)
}

/// repo root → 自動 db パス (`~/.cache/kenning/<name>-<hash8>.db`)。db 管理を意識させない。
pub(crate) fn auto_db_path(root: &std::path::Path) -> Option<String> {
    let cache = cache_dir()?;
    std::fs::create_dir_all(&cache).ok()?;
    let root_s = root.to_string_lossy();
    let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "repo".into());
    Some(cache.join(format!("{}-{:08x}.db", name, hash_u32(&root_s))).to_string_lossy().to_string())
}

/// dir の db パスを解決: env KENNING_DB > repo root からの自動導出。main.rs の index/update 用。
pub fn default_db_for(dir: &str) -> Option<String> {
    if let Ok(v) = std::env::var("KENNING_DB")
        && !v.is_empty() {
            return Some(v);
        }
    repo_root_of(dir).and_then(|r| auto_db_path(&r))
}

/// dir の repo root (文字列)。main.rs の `update` (引数なし) 用。
pub fn repo_root_str(dir: &str) -> Option<String> {
    repo_root_of(dir).map(|p| p.to_string_lossy().to_string())
}

/// Cargo.toml の `[package] name` を読む (簡易 parse、toml crate 依存なし)。
/// virtual workspace manifest ([workspace] だけ) は None。
pub(crate) fn pkg_name_of(cargo_toml: &Path) -> Option<String> {
    let s = std::fs::read_to_string(cargo_toml).ok()?;
    let mut in_pkg = false;
    for line in s.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_pkg = t == "[package]";
            continue;
        }
        if in_pkg
            && let Some(v) = t.strip_prefix("name").and_then(|r| r.trim_start().strip_prefix('=')) {
                let v = v.trim().trim_matches('"');
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
    }
    None
}

/// manifest が `[workspace]` を持つか (簡易 parse)。workspace root を判定する。
pub(crate) fn manifest_has_workspace(cargo_toml: &Path) -> bool {
    std::fs::read_to_string(cargo_toml).is_ok_and(|s| s.lines().any(|l| l.trim() == "[workspace]"))
}

/// root 以下の **独立した cargo project** の manifest dir を列挙 (walk は index_walk = ignore 準拠なので
/// gitignore 済み / 隠し dir の死んだ tree は最初から入らない)。workspace member のように上位 manifest
/// を持つものは、その上位だけを残す (RA が読む単位に合わせる)。
pub(crate) fn cargo_projects_under(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = index_walk(&root.to_string_lossy())
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_name() == "Cargo.toml")
        .filter_map(|e| e.path().parent().map(|p| p.to_path_buf()))
        .collect();
    dirs.sort();
    let mut top: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if !top.iter().any(|t| d.starts_with(t)) {
            top.push(d); // sort 済みなので祖先が先に入る = 子は落ちる
        }
    }
    top
}

/// bake (rust-analyzer scip) に渡すディレクトリを決める。
///
/// RA は渡した path (かその祖先) の Cargo.toml を **1 つだけ**読む。無ければ直下の子を探し、
/// 1 つなら黙ってそれを、複数なら `Error: more than one project` で落ちる。git root をそのまま
/// 渡すと、root が cargo project でない repo では「本命でない crate が黙って焼かれる」か
/// 「関係ない兄弟 project のせいで bake 全体が失敗する」になる (#4)。
///
/// なので cwd を含む **cargo workspace の root** を返す: 最寄り祖先 Cargo.toml から repo root まで
/// 登り、`[workspace]` を持つ最上位があればそれ (member 全体を焼く)、無ければ最寄り。
pub(crate) fn bake_target_of(dir: &str, root: &Path) -> Result<PathBuf, String> {
    let start = std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));
    let root = &std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()); // 比較の前に原点を揃える
    let mut nearest: Option<PathBuf> = None;
    let mut workspace: Option<PathBuf> = None;
    let mut cur = Some(start.as_path());
    while let Some(p) = cur {
        if p.join("Cargo.toml").exists() {
            if nearest.is_none() {
                nearest = Some(p.to_path_buf());
            }
            if manifest_has_workspace(&p.join("Cargo.toml")) {
                workspace = Some(p.to_path_buf()); // 上へ行くほど上書き = 最上位が残る
            }
        }
        if p == root {
            break; // repo の外は見ない
        }
        cur = p.parent();
    }
    if let Some(d) = workspace.or(nearest) {
        return Ok(d);
    }
    // cwd の系統に Cargo.toml が無い repo (crates/ 直下に並ぶ / server+web の混在など)。
    // RA に任せると黙って 1 つ選ぶか落ちるので、こちらで数えて選択を促す。
    let projects = cargo_projects_under(root);
    match projects.len() {
        0 => Err(format!("# bake 対象の cargo project が無い ({})。Cargo.toml のある場所で実行を。", root.display())),
        1 => Ok(projects[0].clone()),
        n => {
            let list = projects.iter().take(8).map(|p| format!("#   {}", p.display())).collect::<Vec<_>>().join("\n");
            Err(format!(
                "# repo に独立した cargo project が {n} 個あり、どれを焼くか決められない:\n{list}\n\
                 # 焼きたい crate へ cd してから `kenning bake` を (rust-analyzer は 1 project しか読めない)。"
            ))
        }
    }
}

/// path から crate 名を解決: 最寄り祖先 Cargo.toml の package name (見つからなければ "root")。
/// workspace の crates/ 配置も root+サブ dir 配置 (ルート crate + ffi/ reader/ 等のサブ crate) も同じ論理。
/// cache は「file の親 dir → crate 名」(同 dir のファイルで stat/read を繰り返さない)。
pub(crate) fn crate_of(path: &str, cache: &mut HashMap<PathBuf, String>) -> String {
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
