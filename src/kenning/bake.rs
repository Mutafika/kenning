//! rust-analyzer をバッチとして 1 回走らせ、SCIP を取り込む。

use super::*;

// ─────────────────────────── bake (RA = オフラインの焼き窯) ───────────────────────────
//
// `kenning bake [dir]` = rust-analyzer scip を回して --scip 再 index まで一発。
// RA は常駐させず、精度が要る時だけバッチで焚く (peak ~5GB × 数十秒、常駐 0)。
// ガード: (i) 空きメモリゲート (足りなければ焚かない) (ii) グローバル lock で直列化
// (iii) features は --config-path で all を注入 (GIGO 対策、Cargo.toml は触らない)。

/// bake 直列化 lock。Drop で必ず解放 (パニック時も unwind で外れる)。
/// SIGTERM/SIGKILL では Drop が走らず残骸化するので、衝突時は lock 内 pid の生存を
/// 確認して、死んでいれば自動回収する (issue #1)。best-effort 直列化なので TOCTOU は許容。
pub(crate) struct BakeLock(pub(crate) std::path::PathBuf);
impl BakeLock {
    pub(crate) fn acquire(cache: &std::path::Path) -> Option<BakeLock> {
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
pub(crate) fn pid_alive(pid: &str) -> bool {
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
pub(crate) fn avail_mem_mb() -> Option<u64> {
    let pct = std::process::Command::new("memory_pressure").arg("-Q").output().ok()?;
    let s = String::from_utf8_lossy(&pct.stdout);
    let pct: u64 = s.lines().find(|l| l.contains("free percentage"))?.trim_end_matches('%').rsplit(' ').next()?.parse().ok()?;
    let total = std::process::Command::new("sysctl").args(["-n", "hw.memsize"]).output().ok()?;
    let total: u64 = String::from_utf8_lossy(&total.stdout).trim().parse().ok()?;
    Some(total / 1_048_576 * pct / 100)
}

/// rust-analyzer binary を探す: env KENNING_RA > PATH > rustup toolchain 直。
pub(crate) fn find_ra() -> Option<String> {
    let mut cands: Vec<String> = Vec::new();
    if let Ok(p) = std::env::var("KENNING_RA") {
        cands.push(p);
    }
    cands.push("rust-analyzer".to_string());
    if let Ok(home) = std::env::var("HOME")
        && let Ok(rd) = std::fs::read_dir(format!("{home}/.rustup/toolchains")) {
            for e in rd.flatten() {
                cands.push(e.path().join("bin/rust-analyzer").to_string_lossy().to_string());
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

/// `/usr/bin/time -l` の統計 (別ファイルに逃がしたもの) から peak RSS を MB で拾う。
pub(crate) fn parse_peak_mb(stats: &str) -> Option<u64> {
    stats
        .lines()
        .find(|l| l.contains("maximum resident set size"))
        .and_then(|l| l.trim().split(' ').next())
        .and_then(|n| n.parse::<u64>().ok())
        .map(|b| b / 1_048_576)
}

/// rust-analyzer の stderr から診断に効く行だけ拾う。
/// panic は「`thread 'main' panicked at ...:`」の**次行**がメッセージ本体なので連れてくる。
/// 何も引っかからなければ末尾 5 行に落とす (従来挙動)。
pub(crate) fn ra_error_lines(errs: &str) -> String {
    const MARK: [&str; 5] = ["panicked", "panic:", "error", "invariant", "fatal"];
    let lines: Vec<&str> = errs.lines().collect();
    let mut take = vec![false; lines.len()];
    for (i, l) in lines.iter().enumerate() {
        let low = l.to_ascii_lowercase();
        if MARK.iter().any(|m| low.contains(m)) {
            take[i] = true;
            if low.contains("panic") && i + 1 < lines.len() {
                take[i + 1] = true;
            }
        }
    }
    let picked: Vec<&str> =
        lines.iter().enumerate().filter(|(i, _)| take[*i]).map(|(_, l)| *l).take(20).collect();
    if !picked.is_empty() {
        return picked.join("\n");
    }
    let mut tail: Vec<&str> = lines.iter().rev().take(5).copied().collect();
    tail.reverse();
    if tail.is_empty() {
        "(rust-analyzer は stderr に何も出さなかった)".to_string()
    } else {
        tail.join("\n")
    }
}

/// rust-analyzer scip の上限秒。features=all は optional dep の build script (bundled C++ / binary DL)
/// で数分かかることがある一方、RA が proc-macro server の応答待ちで永久に固まる事故もある
/// (enchudb で実測: 6 分の build script の後 `load_dylib` の read で停止)。上限で group ごと落として
/// default features に退避し、以後は marker で all を試さない。env KENNING_BAKE_TIMEOUT (秒) で変更可。
pub(crate) const BAKE_TIMEOUT_SECS: u64 = 900;

/// 子を新 process group で起動し、`timeout` で group ごと SIGKILL する (RA が起こした proc-macro
/// server / cargo も道連れ)。stderr は file へ (pipe だと読まない間に詰まって、それ自体が hang になる)。
/// Ok(Some(status)) = 完了、Ok(None) = timeout で kill 済み。
pub(crate) fn run_with_timeout(mut cmd: std::process::Command, err_path: &str, timeout: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
    let err = std::fs::File::create(err_path)?;
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    let t = Instant::now();
    loop {
        if let Some(st) = child.try_wait()? {
            return Ok(Some(st));
        }
        if t.elapsed() >= timeout {
            #[cfg(unix)]
            let _ = std::process::Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
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

    // ── scip の対象は repo root ではなく「cwd を含む cargo workspace」(#4) ──
    // db と index の範囲は repo root のまま = 精密 facts が workspace、syn 層が repo 全体。
    let bake_dir = match bake_target_of(dir, &root) {
        Ok(d) => d,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    let bake_s = bake_dir.to_string_lossy().to_string();
    if bake_s != root_s {
        let others: Vec<String> = cargo_projects_under(&root)
            .into_iter()
            .filter(|p| !p.starts_with(&bake_dir))
            .map(|p| rel_of(&p.to_string_lossy(), &root_s))
            .collect();
        eprintln!("# bake 対象: {bake_s} (repo root ではなく cwd の cargo workspace)");
        if !others.is_empty() {
            eprintln!("# ⚠ workspace 外の cargo project {} 個 ({}) は精密 facts の対象外 (syn 層のまま)。", others.len(), others.join(", "));
        }
    }

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
    // 節目は**焼き始め**の時点で取る。完了時に取ると、焼いている間の vup を「焼き済み」と記録してしまい、
    // その分が次の判定で焼き直されない。
    let milestone_at_start = milestone_id(&root_s, &db);

    let Some(ra) = find_ra() else {
        eprintln!("# rust-analyzer が見つからない。`rustup component add rust-analyzer` か env KENNING_RA で指定を。");
        std::process::exit(2);
    };

    // ── SCIP 生成 (features=all を --config-path で注入。Cargo.toml は触らない) ──
    let scip_path = format!("{}.scip", db.trim_end_matches(".db"));
    let cfg_path = format!("{}.racfg.json", db.trim_end_matches(".db"));
    // time(1) の統計は -o で別ファイルへ逃がす。子の stderr を汚さない = 失敗時に
    // rust-analyzer の panic/error がそのまま読める (末尾 5 行が time のフッターに潰されない)。
    let stats_path = format!("{}.time", db.trim_end_matches(".db"));
    let err_path = format!("{db}.ra-err"); // `<db>.` prefix = cache prune の道連れ対象
    let use_time = std::path::Path::new("/usr/bin/time").exists();
    // features=all が timeout / 失敗した repo は marker を残し、次回から default で焼く (毎回 15 分払わない)。
    let default_marker = format!("{db}.bake-default");
    let timeout = std::env::var("KENNING_BAKE_TIMEOUT").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(BAKE_TIMEOUT_SECS);
    let mut use_all = std::env::var_os("KENNING_BAKE_DEFAULT_FEATURES").is_none();
    if use_all && std::path::Path::new(&default_marker).exists() {
        eprintln!("# 前回 features=all が失敗 / timeout → default features で焼く (all を試し直すなら rm {default_marker})");
        use_all = false;
    }
    let n_src = rust_files(&bake_s).count(); // 「薄い SCIP」判定は bake 対象の規模と比べる
    let mut peak_mb = 0u64;
    let mut baked = false;
    // repo が RUSTFLAGS で custom cfg を要求する形 (tokio の `--cfg tokio_unstable` など) は
    // Cargo.toml に現れないので当てられない。**repo 側の事情は repo を知っている人が渡す** —
    // 渡された分だけ RA に注入する (実測: tokio で確定 11,235 → 12,332、59.2% → 65.0%)。
    let rustflags = std::env::var("KENNING_BAKE_RUSTFLAGS").unwrap_or_default();
    if !rustflags.is_empty() {
        eprintln!("# bake: RUSTFLAGS={rustflags} を RA に注入 (KENNING_BAKE_RUSTFLAGS)");
    }
    // RA が読む前の内容を記録 (この後で編集された file は、再利用時に SCIP を使わない)。
    write_scip_src(&scip_path, &root_s);
    for attempt in 0..2 {
        let all = use_all && attempt == 0;
        // features=all も RUSTFLAGS も無い時だけ config を書かない (= RA の既定で焚く)。
        if all || !rustflags.is_empty() {
            std::fs::write(&cfg_path, ra_config(all, &rustflags)).unwrap();
        }
        eprintln!(
            "# bake: rust-analyzer scip {} (features={}) — peak ~{:.1}GB / 数十秒〜数分 (上限 {timeout}s)、常駐なし",
            bake_s, if all { "all" } else { "default" }, needed_mb as f64 / 1024.0
        );
        let _ = std::fs::remove_file(&stats_path);
        let mut cmd = if use_time {
            let mut c = std::process::Command::new("/usr/bin/time");
            c.args(["-l", "-o", &stats_path, &ra]);
            c
        } else {
            std::process::Command::new(&ra)
        };
        cmd.args(["scip", &bake_s, "--output", &scip_path]);
        if all || !rustflags.is_empty() {
            cmd.args(["--config-path", &cfg_path]);
        }
        cmd.current_dir(&bake_s);
        let t_ra = Instant::now();
        let status = match run_with_timeout(cmd, &err_path, Duration::from_secs(timeout)) {
            Ok(st) => st,
            Err(e) => {
                eprintln!("# bake 失敗: rust-analyzer を起動できない ({ra}): {e}");
                std::process::exit(1);
            }
        };
        let errs = std::fs::read_to_string(&err_path).unwrap_or_default();
        let _ = std::fs::remove_file(&err_path);
        peak_mb = std::fs::read_to_string(&stats_path).ok().and_then(|s| parse_peak_mb(&s)).unwrap_or(0);
        let Some(status) = status else {
            eprintln!(
                "# ⚠ rust-analyzer が {timeout}s で終わらない (features={}) → 止めた。optional dep の build script が重いか、proc-macro server の応答待ちで固まった疑い",
                if all { "all" } else { "default" }
            );
            if all {
                let _ = std::fs::write(&default_marker, "");
                eprintln!("# default features で焼き直す (次回からも default。KENNING_BAKE_TIMEOUT=<秒> で上限変更)");
                continue;
            }
            eprintln!("# 真因を直に見るなら: (cd {bake_s} && {ra} scip .)");
            std::process::exit(1);
        };
        if !status.success() || !std::path::Path::new(&scip_path).exists() {
            eprintln!("# bake 失敗 (features={}, {:.0?}):", if all { "all" } else { "default" }, t_ra.elapsed());
            eprintln!("{}", ra_error_lines(&errs));
            if all {
                let _ = std::fs::write(&default_marker, "");
                continue;
            }
            eprintln!("# 真因を直に見るなら: (cd {bake_s} && {ra} scip .)");
            std::process::exit(1);
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
        eprintln!("# bake 完了: {} docs / peak {}MB / {:.0?}", n_docs, peak_mb, t_ra.elapsed());
        baked = true;
        break;
    }
    if !baked {
        eprintln!("# bake 失敗 (all/default 両方)");
        eprintln!("# 真因を直に見るなら: (cd {bake_s} && {ra} scip .)");
        std::process::exit(1);
    }
    let _ = std::fs::remove_file(&cfg_path);
    let _ = std::fs::remove_file(&stats_path);

    // ── SCIP 込みで full 再 index → meta に bake 情報を焼く ──
    run_index(&root_s, &db, Some(&scip_path));
    if let Ok(dbw) = Database::open(&db)
        && let Some(mt) = dbw.get_table("meta") {
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
    // 自動 bake が同じ workspace を焼けるように覚えておく (root 直下が cargo project でない repo 用)。
    let _ = std::fs::write(bake_dir_marker(&db), &bake_s);
    // この bake が「どの節目 (vup / commit) の時点か」を覚える。次にこれが進んだら焼き直す。
    if let Some(m) = milestone_at_start {
        let _ = std::fs::write(bake_milestone_marker(&db), m);
    }
    eprintln!("# 精密 facts 有効: refs / callers が RA 同等精度に (`kenning refs <name>`)");
}

// ── 自動 bake: 精密 facts の鮮度を常駐なしで保つ ──
// 活発な repo では bake の効きが数日で消える (enchudb 実測: 確定率 80.2% → 193 file 変更後 18.5%)。
// RA を常駐させずに保つため、増分 update が「古い」と判定した時に、空いていれば裏で bake を切り離して
// 起動する。query 自体は待たない。対象は一度手で bake した repo だけ (勝手に GB 級を焚き始めない)。

/// 同じ repo で自動 bake を起こす最短間隔。
pub(crate) const AUTO_BAKE_INTERVAL_SECS: u64 = 30 * 60;
/// 1 CPU あたりの load がこれ以上なら見送る (並列 session で埋まっている時に 2GB 級を足さない)。
pub(crate) const AUTO_BAKE_MAX_LOAD_PER_CPU: f64 = 1.0;

pub(crate) fn bake_dir_marker(db: &str) -> String {
    format!("{db}.bake-dir")
}
fn auto_bake_log(db: &str) -> String {
    format!("{db}.auto-bake.log") // `<db>.` prefix = cache prune の道連れ対象
}

/// 自動 bake の可否。
#[derive(Debug, PartialEq)]
pub(crate) enum AutoBake {
    Off,
    /// 前回起動からまだ間隔が空いていない (残り秒)。
    Throttled(u64),
    /// 別の bake (別 project 含む。lock はマシンに 1 つ) が進行中。間隔の時計は進めず、次の query で再判定。
    LockBusy,
    /// 負荷が高い (1 CPU あたりの load)。
    Busy(f64),
    Go,
}

/// 判定だけ (副作用なし = テスト可能)。`since_last` = 前回起動からの秒、`lock_busy` = 別の bake が進行中、
/// `load` = 1 CPU あたりの load。
pub(crate) fn auto_bake_gate(off: bool, since_last: Option<u64>, lock_busy: bool, load: Option<f64>) -> AutoBake {
    if off {
        return AutoBake::Off;
    }
    if let Some(s) = since_last.filter(|s| *s < AUTO_BAKE_INTERVAL_SECS) {
        return AutoBake::Throttled(AUTO_BAKE_INTERVAL_SECS - s);
    }
    if lock_busy {
        return AutoBake::LockBusy;
    }
    match load {
        Some(l) if l >= AUTO_BAKE_MAX_LOAD_PER_CPU => AutoBake::Busy(l),
        _ => AutoBake::Go, // 測れない時は bake 側の空きメモリゲートに任せる
    }
}

/// マシン共通の bake lock (`<cache>/bake.lock`) を生きた process が持っていればその pid。
/// 死んだ残骸は「空き」扱い (bake 側が回収する)。
fn bake_lock_holder(db: &str) -> Option<String> {
    let cache = Path::new(db).parent()?;
    let pid = std::fs::read_to_string(cache.join("bake.lock")).ok()?.trim().to_string();
    pid_alive(&pid).then_some(pid)
}

/// `KENNING_AUTO_BAKE=0` (または全自動停止 `KENNING_NO_AUTO`) で無効。自動 bake の子自身も起こさない。
pub(crate) fn auto_bake_off() -> bool {
    std::env::var("KENNING_AUTO_BAKE").is_ok_and(|v| v == "0")
        || std::env::var_os("KENNING_NO_AUTO").is_some()
        || std::env::var_os("KENNING_AUTO_BAKE_CHILD").is_some()
}

/// `{ 8.21 8.46 8.77 }` (macOS sysctl) / `8.21 8.46 8.77 3/900 123` (Linux /proc/loadavg) の 1 分値。
pub(crate) fn parse_load1(s: &str) -> Option<f64> {
    s.split_whitespace().find(|t| *t != "{").and_then(|t| t.parse().ok())
}

fn load_per_cpu() -> Option<f64> {
    let raw = std::fs::read_to_string("/proc/loadavg").ok().or_else(|| {
        let out = std::process::Command::new("sysctl").args(["-n", "vm.loadavg"]).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    })?;
    let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    parse_load1(&raw).map(|l| l / ncpu)
}

/// 増分 update が「SCIP facts が古い」と判定した時に呼ぶ。告知 1 行 + 条件が揃えば裏で bake。
pub(crate) fn maybe_auto_bake(db: &str, dir: &str, stale: u32) {
    auto_bake_with(db, dir, format!("# SCIP facts が古くなってきた (bake 後 {stale} ファイル変更)"));
}

/// 1 process で判定・告知は 1 回だけ (節目の検知と閾値の検知が同じ query で重なっても 2 行出さない)。
static AUTO_BAKE_DECIDED: AtomicBool = AtomicBool::new(false);

fn auto_bake_with(db: &str, dir: &str, head: String) {
    if AUTO_BAKE_DECIDED.swap(true, Ordering::Relaxed) {
        return;
    }
    let log = auto_bake_log(db);
    let since_last = std::fs::metadata(&log).ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).map(|d| d.as_secs());
    let off = auto_bake_off();
    let busy_pid = if off { None } else { bake_lock_holder(db) };
    match auto_bake_gate(off, since_last, busy_pid.is_some(), if off { None } else { load_per_cpu() }) {
        AutoBake::Off => eprintln!("{head} → `kenning bake` 推奨 (自動 bake は無効: KENNING_AUTO_BAKE=0)"),
        AutoBake::Throttled(rest) => eprintln!("{head} → 自動 bake は起動済み / 間隔待ち (あと {} 分、ログ {log})", rest.div_ceil(60)),
        // 起動しない = log を作らない = 30 分の時計を進めない → 空いたら次の query ですぐ焼く (列は作らない)。
        AutoBake::LockBusy => eprintln!("{head} → 別の bake が進行中 (pid {}) → 空いたら次の query で起動", busy_pid.unwrap_or_default()),
        AutoBake::Busy(l) => eprintln!("{head} → 負荷が高い (load/CPU {l:.2}) ので自動 bake は見送り。`kenning bake` 推奨"),
        AutoBake::Go => {
            let target = std::fs::read_to_string(bake_dir_marker(db)).ok().map(|s| s.trim().to_string()).filter(|s| Path::new(s).is_dir()).unwrap_or_else(|| dir.to_string());
            match spawn_auto_bake(&target, &log) {
                Ok(()) => eprintln!("{head} → 裏で bake を起動 (nice、ログ {log}。無効化は KENNING_AUTO_BAKE=0)"),
                Err(e) => eprintln!("{head} → 自動 bake を起動できない ({e})。`kenning bake` 推奨"),
            }
        }
    }
}

// ── 節目 (vup / commit) で焼き直す ──
// bake のきっかけは「20 ファイル変わった」より「作業の節目」の方が筋が良い: 焼く時点が揃い、
// 節目同士の比較 (`changes`) の精度も揃う。VCS に呼んでもらうのではなく kenning が状態を読みに行く
// (git: .git の HEAD、sinfo: module ごとの current = `sf log --heads --json`)。閾値の方は保険として残る。

pub(crate) fn bake_milestone_marker(db: &str) -> String {
    format!("{db}.bake-milestone")
}

/// 節目の ID。git と sinfo の両方を使う repo は両方を連結 (どちらが進んでも変わる)。どちらも無ければ None。
pub(crate) fn milestone_id(root: &str, db: &str) -> Option<String> {
    let parts: Vec<String> = [git_head(root).map(|h| format!("git:{h}")), sinfo_heads(root, db).map(|h| format!("sinfo:{h}"))]
        .into_iter()
        .flatten()
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// `.git/HEAD` を直接読む (process を起こさない、µs)。ref は loose → packed-refs の順。
/// worktree (`.git` が file) や読めない形は `git rev-parse` に落とす。
fn git_head(root: &str) -> Option<String> {
    let g = Path::new(root).join(".git");
    if g.is_dir() {
        let head = std::fs::read_to_string(g.join("HEAD")).ok()?;
        let head = head.trim();
        let Some(r) = head.strip_prefix("ref: ") else { return Some(head.to_string()) };
        if let Ok(h) = std::fs::read_to_string(g.join(r)) {
            return Some(h.trim().to_string());
        }
        if let Some(h) = std::fs::read_to_string(g.join("packed-refs")).ok().and_then(|p| {
            p.lines().find_map(|l| l.strip_suffix(r).map(|x| x.trim().to_string()))
        }) {
            return Some(h);
        }
    } else if !g.exists() {
        return None;
    }
    let out = std::process::Command::new("git").arg("-C").arg(root).args(["rev-parse", "HEAD"]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// sinfo の module ごとの current の指紋。`sf` を毎 query 起こさないよう、`.sinfo/db` の中身の最新
/// mtime が前回と同じなら `<db>.sinfo-heads` に覚えた指紋を返す (vup すれば db が書かれて mtime が動く)。
fn sinfo_heads(root: &str, db: &str) -> Option<String> {
    let sdb = Path::new(root).join(".sinfo").join("db");
    if !sdb.is_dir() {
        return None;
    }
    let mtime = newest_mtime_ns(&sdb)?;
    let memo = format!("{db}.sinfo-heads");
    if let Some((m, h)) = std::fs::read_to_string(&memo).ok().and_then(|s| s.trim().split_once('\t').map(|(a, b)| (a.to_string(), b.to_string())))
        && m == mtime.to_string()
    {
        return Some(h);
    }
    let out = std::process::Command::new("sf").args(["log", "--heads", "--json"]).current_dir(root).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let h = format!("{:016x}", fnv1a(&out.stdout));
    let _ = std::fs::write(&memo, format!("{mtime}\t{h}"));
    Some(h)
}

fn newest_mtime_ns(dir: &Path) -> Option<u128> {
    let mut best: Option<u128> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(e.path());
            }
            if let Some(t) = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()) {
                best = Some(best.map_or(t.as_nanos(), |b| b.max(t.as_nanos())));
            }
        }
    }
    best
}

/// 64-bit FNV-1a (指紋用。暗号強度は要らず、依存を足さない)。
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// query ごと (鮮度確認の直前) に呼ぶ。bake 済みで変更があり、節目が bake 時点から進んでいれば焼き直す。
/// 節目の記録が無い (この機能より前の bake) なら今の節目を記録するだけ — 入れた途端に全 repo を焼かない。
pub(crate) fn maybe_milestone_bake(db: &str, root: &str, upd_since_bake: u32) {
    if upd_since_bake == 0 {
        return; // 節目が進んでも中身が bake 時点と同じなら焼き直す意味が無い
    }
    let Some(now) = milestone_id(root, db) else { return };
    let marker = bake_milestone_marker(db);
    match std::fs::read_to_string(&marker) {
        Ok(prev) if prev.trim() == now => {}
        Ok(_) => auto_bake_with(db, root, format!("# vup / commit が bake 以降に進んだ (bake 後 {upd_since_bake} ファイル変更)")),
        Err(_) => {
            let _ = std::fs::write(&marker, now);
        }
    }
}

/// 自分自身の `bake <dir>` を低優先度・別 process group で切り離す (query は待たない。端末の Ctrl-C も届かない)。
fn spawn_auto_bake(target: &str, log: &str) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let out = std::fs::File::create(log)?; // 作成 = 起動時刻 (間隔判定は log の mtime)
    let mut cmd = std::process::Command::new("nice");
    cmd.args(["-n", "10"]).arg(exe).arg("bake").arg(target)
        .env("KENNING_AUTO_BAKE_CHILD", "1")
        .stdin(std::process::Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(|_| ())
}

/// RA の `--config-path` に渡す JSON。features=all と RUSTFLAGS を必要な分だけ載せる
/// (空のキーを書くと RA 側の既定を上書きしてしまうので、立っている物だけ書く)。
pub(crate) fn ra_config(all: bool, rustflags: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if all {
        parts.push(r#""features": "all""#.to_string());
    }
    if !rustflags.is_empty() {
        parts.push(format!(r#""extraEnv": {{"RUSTFLAGS": {}}}"#, json_str(rustflags)));
    }
    format!(r#"{{"cargo": {{{}}}}}"#, parts.join(", "))
}

/// JSON 文字列リテラル化 (RUSTFLAGS に " や \ が入っても config を壊さない)。
pub(crate) fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
