//! 自動導出 db の棚卸しと掃除 (~/.cache/kenning)。

use super::*;

// ---------------------------------------------------------------------------
// cache — 自動導出 db の棚卸しと掃除 (~/.cache/kenning)
// ---------------------------------------------------------------------------
// db は repo ごとに勝手に増える派生物で、消えた repo / 旧版 / 旧 format の index が居座る
// (v9 実測 21 db で実 1.2GB・apparent 41GB。v10 で apparent は ~6 分の 1)。
// 「消していい物」だけ機械的に選べるようにする。

/// 1 index 分の観測 (`cache ls|prune` 用)。
pub(crate) struct CacheEntry {
    pub(crate) db: PathBuf,
    root: String, // meta.root。旧版 (meta 無し) は空
    built_at: u32,
    pub(crate) bytes: u64, // sidecar 込みの実消費 (sparse の穴は数えない)
    pub(crate) status: CacheStatus,
}

#[derive(PartialEq)]
pub(crate) enum CacheStatus {
    Ok,
    RootMissing, // repo を移動/削除 → 二度と使われない
    NoMeta,      // 旧版 (次のクエリで自動 heal されるが、その repo を触らなければ残る)
    LegacyFile,  // enchudb v9 以前の 1 ファイル db。v10 は必ず directory なので構造だけで判る
    Unreadable,  // open 失敗 (壊れている or 書込み中)。安全側で自動削除の対象外
}

impl CacheStatus {
    fn label(&self) -> &'static str {
        match self {
            CacheStatus::Ok => "ok",
            CacheStatus::RootMissing => "root missing",
            CacheStatus::NoMeta => "旧版 (meta 無し)",
            CacheStatus::LegacyFile => "旧 format (v9 単一ファイル)",
            CacheStatus::Unreadable => "開けない (要手動確認)",
        }
    }
}

/// db に付随するもの (v10 では `X.db` が directory で enchudb の sidecar はその中。v9 以前が
/// 隣に残した `X.db.oplog` 等と、bake 済み `X.scip`)。prefix で拾うので取りこぼさない。
pub(crate) fn cache_sidecars(db: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (db.parent(), db.file_name().map(|n| n.to_string_lossy().to_string())) else {
        return vec![db.to_path_buf()];
    };
    let scip = scip_path_of(&name);
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    n == name || n.starts_with(&format!("{name}.")) || n == scip
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// 実消費バイト数 (sparse の穴を数えない)。apparent size は enchudb の疎 stride で数十倍に見える。
/// v10 の db は directory (`himo/NNNN.seg` と入れ子) なので再帰して合算する。
pub(crate) fn real_bytes(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    #[cfg(unix)]
    let own = {
        use std::os::unix::fs::MetadataExt;
        m.blocks() * 512
    };
    #[cfg(not(unix))]
    let own = m.len();
    if !m.is_dir() {
        return own;
    }
    own + std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| real_bytes(&e.path())).sum()).unwrap_or(0)
}

/// cache dir 内の全 index を観測 (db パス順 = 決定的)。
pub(crate) fn cache_entries(cache: &Path) -> Vec<CacheEntry> {
    let mut dbs: Vec<PathBuf> = std::fs::read_dir(cache)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "db")).collect())
        .unwrap_or_default();
    dbs.sort();
    dbs.into_iter()
        .map(|db| {
            let bytes = cache_sidecars(&db).iter().map(|p| real_bytes(p)).sum();
            // v10 の db は必ず directory。単一ファイルなら open するまでもなく v9 以前と確定する
            // (エラー文字列の照合より構造で判る方が壊れにくい)。
            let (root, built_at, status) = match Database::open_readonly(&db.to_string_lossy()) {
                Err(_) if db.is_file() => (String::new(), 0, CacheStatus::LegacyFile),
                Err(_) => (String::new(), 0, CacheStatus::Unreadable),
                Ok(d) => match read_meta(&d) {
                    None => (String::new(), 0, CacheStatus::NoMeta),
                    Some((r, b)) if r.is_empty() || b == 0 => (r, b, CacheStatus::NoMeta),
                    Some((r, b)) => {
                        let st = if Path::new(&r).is_dir() { CacheStatus::Ok } else { CacheStatus::RootMissing };
                        (r, b, st)
                    }
                },
            };
            CacheEntry { db, root, built_at, bytes, status }
        })
        .collect()
}

/// prune 対象か: root 消失 / 旧版 / 旧 format は無条件、`older_than` 日以上前の index も (指定時)。
/// 開けない db は壊れているのか書込み中なのか区別できないので自動では消さない。
pub(crate) fn cache_prunable(e: &CacheEntry, older_than: Option<u32>) -> Option<String> {
    match e.status {
        CacheStatus::RootMissing | CacheStatus::NoMeta | CacheStatus::LegacyFile => Some(e.status.label().to_string()),
        CacheStatus::Unreadable => None,
        CacheStatus::Ok => older_than
            .filter(|d| age_days(e.built_at) >= *d)
            .map(|d| format!("{} 日前 (>= {d})", age_days(e.built_at))),
    }
}

pub(crate) fn mb(bytes: u64) -> String {
    format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
}

/// `cache ls` — 棚卸し。`path TAB 実サイズ TAB 経過日 TAB root TAB 状態`。
pub(crate) fn cache_ls(cache: &Path) {
    let entries = cache_entries(cache);
    if entries.is_empty() {
        println!("# cache に index が無い ({})。各 repo で一度 kenning を叩くと増える。", cache.display());
        return;
    }
    let total: u64 = entries.iter().map(|e| e.bytes).sum();
    let prunable = entries.iter().filter(|e| cache_prunable(e, None).is_some()).count();
    for e in &entries {
        let age = if e.built_at == 0 { "?".to_string() } else { format!("{}d", age_days(e.built_at)) };
        let root = if e.root.is_empty() { "?" } else { e.root.as_str() };
        println!("{}\t{}\t{}\t{}\t{}", e.db.display(), mb(e.bytes), age, root, e.status.label());
    }
    println!(
        "# {} index / {} (実消費)。掃除できる: {prunable} (root 消失 / 旧版 / 旧 format)。`kenning cache prune [--older-than 日数] [--dry-run]`",
        entries.len(),
        mb(total)
    );
}

/// `cache prune` — 掃除。消したものを stdout に 1 行ずつ (dry-run は「予定」)。
/// 戻り値 = 消した (または消す予定の) index 数。
pub(crate) fn cache_prune(cache: &Path, older_than: Option<u32>, dry_run: bool) -> usize {
    let entries = cache_entries(cache);
    let mut n = 0;
    let mut freed = 0u64;
    for e in &entries {
        let Some(why) = cache_prunable(e, older_than) else { continue };
        n += 1;
        freed += e.bytes;
        let verb = if dry_run { "削除予定" } else { "削除" };
        println!("{}\t{verb} ({why}, {})", e.db.display(), mb(e.bytes));
        if !dry_run {
            // 旧 format は repo が生きている = 次に触った時に heal で焼き直される。bake 済みの
            // `.scip` は高価 (RA を数十秒 / peak ~5GB) なので、その焼き直しで再利用できるよう残す。
            // root 消失 / 旧版は repo ごと無いので .scip も道連れでいい。
            let keep = (e.status == CacheStatus::LegacyFile).then(|| PathBuf::from(scip_path_of(&e.db.to_string_lossy())));
            for p in cache_sidecars(&e.db) {
                if Some(&p) == keep.as_ref() {
                    continue;
                }
                let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
                if let Err(err) = r {
                    eprintln!("# ⚠ 消せない {}: {err}", p.display());
                }
            }
        }
    }
    let kept = entries.len() - n;
    if dry_run {
        println!("# {n} index / {} を回収予定 (残 {kept})。実行は --dry-run を外す。", mb(freed));
    } else {
        println!("# {n} index / {} を回収 (残 {kept})。", mb(freed));
    }
    n
}

/// `cache [ls|prune] [--older-than <日数>] [--dry-run]` — 自動導出 db の棚卸し/掃除。
/// db を使わない (cache 全走査) ので parse_opts の auto 魔法は通さない。
pub fn cmd_cache(args: &[String]) {
    let mut sub = "ls";
    let mut older_than: Option<u32> = None;
    let mut dry_run = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "ls" | "prune" => sub = if args[i] == "ls" { "ls" } else { "prune" },
            "--older-than" => {
                i += 1;
                older_than = args.get(i).and_then(|v| v.parse().ok());
                if older_than.is_none() {
                    eprintln!("usage: --older-than <日数>");
                    std::process::exit(2);
                }
            }
            "--dry-run" => dry_run = true,
            other => {
                eprintln!("usage: kenning cache [ls|prune] [--older-than <日数>] [--dry-run] (不明: {other})");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let Some(cache) = cache_dir() else {
        eprintln!("# HOME が無いので ~/.cache/kenning を特定できない。");
        std::process::exit(2);
    };
    match sub {
        "prune" => {
            cache_prune(&cache, older_than, dry_run);
        }
        _ => cache_ls(&cache),
    }
}
