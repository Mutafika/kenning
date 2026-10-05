//! `changes` — 前回の snapshot からの**意味的な差分** (壊れた参照 / シグネチャ変更 / 新しく dead /
//! 復活 / 呼び元数の増減 — 行は `--all` の時だけ)。読み手を選ばない: 既定は `path:line<TAB>種類<TAB>詳細` (人間・Claude・
//! エディタ)、`--json` で NDJSON (CI・スクリプト)。
//!
//! snapshot は db の隣 `<db>.changes/<token>.tsv` に置く (db 本体の schema は触らない = INDEX_VER 不変、
//! `cache prune` は prefix で一緒に回収する)。key は eid ではなく `crate / module / kind / Container::name`
//! なので、full 再 index を挟んでも比べられる。対象は fn / method だけ (call edge を持つのはこれだけ)。
//!
//! 起点の渡し方は 3 つ: `--since <git ref>` (状態なし、`HEAD` = commit していない作業の差分)、
//! `--since <token>` (呼び手が token を持つ)、 `--cursor <name>` (名前付きで
//! kenning 側が token を進める = hook のように状態を持てない呼び手向け)。どちらも無ければ baseline を
//! 作って token だけ返す。

use super::*;

/// 消さずに残す snapshot の数 (cursor が指している物は別枠で必ず残す)。
pub(crate) const CHANGES_KEEP: usize = 8;
/// この日数 使われていない cursor は捨てる (hook は session ごとに cursor を作るので、終わった session の
/// cursor が snapshot を pin したまま溜まり続けないように)。
pub(crate) const CURSOR_TTL_DAYS: u64 = 7;
const SNAP_EXT: &str = "tsv";
/// snapshot の形式 (key の振り方を含む)。変えたら上げる — 旧形式の snapshot と比べると key がずれて
/// 偽の差分になるので、読まずに捨てる (git ref の cache は焼き直し、token は「無い」扱い)。
const SNAP_HEADER: &str = "# kenning-changes snapshot v3";
/// 2 行目: 採取時点の bake 時刻 (0 = syn 層だけ)。間に bake が入った 2 つは精度が違う。
const BAKED_PREFIX: &str = "# baked_at ";
const CURSOR_PREFIX: &str = "cursor-";

/// fn / method 1 件分の観測。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SnapRow {
    pub(crate) qual: String,
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) sig: String,
    pub(crate) dead: bool,
    pub(crate) callers: u32,
}

/// key → 観測。BTreeMap = 出力順が決定的。
pub(crate) type Snapshot = std::collections::BTreeMap<String, SnapRow>;
/// 名前 → 呼び出し位置 (path, line)。消えた定義の呼び出しが残っているかを見る。
pub(crate) type CallSites = HashMap<String, Vec<(String, u32)>>;

/// 差分 1 件。`kind` の並び = 深刻な順 (出力もこの順)。
#[derive(Debug, PartialEq)]
pub(crate) enum Change {
    /// 定義が消えたのに、その名前の呼び出しが残っている (at = 残っている呼び出しの 1 つ)。
    Broken { qual: String, at: (String, u32), remaining: u32 },
    Sig { qual: String, at: (String, u32), old: String, new: String, callers: u32 },
    /// `added` = 今回足された定義 (足したのに誰も呼んでいない = 繋ぎ忘れ)。
    Dead { qual: String, at: (String, u32), added: bool },
    Revived { qual: String, at: (String, u32) },
    /// `dup` = 同名の定義が増えた (呼び出しの解決が曖昧になって確定を失った可能性)。
    Callers { qual: String, at: (String, u32), old: u32, new: u32, dup: bool },
}

impl Change {
    fn kind(&self) -> &'static str {
        match self {
            Change::Broken { .. } => "broken",
            Change::Sig { .. } => "sig",
            Change::Dead { .. } => "dead",
            Change::Revived { .. } => "revived",
            Change::Callers { .. } => "callers",
        }
    }
    fn at(&self) -> &(String, u32) {
        match self {
            Change::Broken { at, .. } | Change::Sig { at, .. } | Change::Dead { at, .. } | Change::Revived { at, .. } | Change::Callers { at, .. } => at,
        }
    }
    fn qual(&self) -> &str {
        match self {
            Change::Broken { qual, .. } | Change::Sig { qual, .. } | Change::Dead { qual, .. } | Change::Revived { qual, .. } | Change::Callers { qual, .. } => qual,
        }
    }
    fn detail(&self) -> String {
        match self {
            Change::Broken { qual, remaining, .. } => format!("{qual} の定義が消えたが、呼び出しが {remaining} 件残る"),
            Change::Sig { qual, old, new, callers, .. } => format!("{qual}: {old} → {new}  (callers {callers})"),
            Change::Dead { qual, added: false, .. } => format!("{qual} が live root から届かなくなった"),
            Change::Dead { qual, added: true, .. } => format!("{qual} を足したが live root から届かない (繋ぎ忘れ?)"),
            Change::Revived { qual, .. } => format!("{qual} がまた届くようになった"),
            Change::Callers { qual, old, new: 0, dup: true, .. } => format!("{qual}: callers {old} → 0 (同名の定義が増えた — 呼び出しの解決が曖昧になった。重複定義?)"),
            Change::Callers { qual, old, new: 0, .. } => format!("{qual}: callers {old} → 0 (確定の呼び元が無くなった)"),
            Change::Callers { qual, old, new, .. } => format!("{qual}: callers {old} → {new}"),
        }
    }
    fn json(&self) -> String {
        let (p, l) = self.at();
        let mut s = format!("{{\"kind\":\"{}\",\"path\":{},\"line\":{l},\"symbol\":{}", self.kind(), json_str(p), json_str(self.qual()));
        match self {
            Change::Broken { remaining, .. } => s += &format!(",\"remaining\":{remaining}"),
            Change::Sig { old, new, callers, .. } => s += &format!(",\"old\":{},\"new\":{},\"callers\":{callers}", json_str(old), json_str(new)),
            Change::Callers { old, new, dup, .. } => s += &format!(",\"old\":{old},\"new\":{new},\"dup\":{dup}"),
            Change::Dead { added, .. } => s += &format!(",\"added\":{added}"),
            Change::Revived { .. } => {}
        }
        s + "}"
    }
}

/// TSV の 1 セルに入れられる形 (tab / 改行を潰す)。
fn cell(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// 今の index から snapshot を採る。dead は `search reachable:0` と同じ判定 (字句照合込み = 安全側)。
/// 同時に「名前 → 呼び出し位置」も返す (消えた定義の呼び出しが残っているかを見るため)。
pub(crate) fn take_snapshot(db: &Database) -> (Snapshot, CallSites) {
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    let mut dead = dead_by_elimination(&call_t, &sym_t);
    if !dead.is_empty() {
        let cand: Vec<EntityId> = dead.iter().copied().collect();
        let used = names_used_lexically(&paths, &sym_t, &cand, &dead);
        dead.retain(|&e| !used.contains(&txt(sym_t.entity(e).get("name"))));
    }

    // call 表を 1 パス: 確定 callers 数 (callee_sym 逆引き) と、名前 → 呼び出し位置。
    let mut callers: HashMap<EntityId, u32> = HashMap::new();
    let mut sites = CallSites::new();
    for c in call_t.all().find().unwrap_or_default() {
        let er = call_t.entity(c);
        if let Some(Value::Ref(t)) = er.get("callee_sym") {
            *callers.entry(t).or_default() += 1;
        }
        let p = paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default();
        sites.entry(txt(er.get("callee"))).or_default().push((p, num(er.get("line"))));
    }
    for v in sites.values_mut() {
        v.sort();
    }

    let mut rows: Vec<(String, SnapRow)> = Vec::new();
    for kind in [K_FN, K_METHOD] {
        for e in sym_t.where_eq("kind", kind).find().unwrap_or_default() {
            let er = sym_t.entity(e);
            let qual = sym_qual(&sym_t, e);
            let base = format!("{}\t{}\t{}\t{qual}", txt(er.get("crate_")), txt(er.get("module")), kind_name(kind));
            rows.push((base, SnapRow {
                qual,
                path: paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(),
                line: num(er.get("line")),
                sig: txt(er.get("sig")),
                dead: dead.contains(&e),
                callers: callers.get(&e).copied().unwrap_or(0),
            }));
        }
    }
    (assign_keys(rows), sites)
}

/// key を振る。名前が一意なら key に path を含めない (file を移しても同じ定義として追える)。
/// 衝突した時 (tests/*.rs ごとの `tmp()`、cfg 分岐の双子) だけ path で区別し、同じ file 内の重複は
/// 行順の通し番号で。**全体の通し番号にはしない** — path 順で間に file が 1 つ入るだけで番号がずれ、
/// 別 file の同名同士を突き合わせて偽の sig を出す (enchudb の作業ツリーで実際に 3 件出た)。
fn assign_keys(mut rows: Vec<(String, SnapRow)>) -> Snapshot {
    rows.sort_by(|a, b| (&a.0, &a.1.path, a.1.line).cmp(&(&b.0, &b.1.path, b.1.line)));
    let mut n_base: HashMap<String, usize> = HashMap::new();
    for (b, _) in &rows {
        *n_base.entry(b.clone()).or_default() += 1;
    }
    let mut snap = Snapshot::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (base, row) in rows {
        let key = if n_base[&base] == 1 {
            base
        } else {
            let k = format!("{base}\t@{}", row.path);
            let i = seen.entry(k.clone()).or_default();
            *i += 1;
            if *i == 1 { k } else { format!("{k}#{i}") }
        };
        snap.insert(key, row);
    }
    snap
}

pub(crate) fn write_snapshot(path: &Path, snap: &Snapshot, baked_at: u32) -> std::io::Result<()> {
    let mut s = format!("{SNAP_HEADER}\n{BAKED_PREFIX}{baked_at}\n");
    for (k, r) in snap {
        s += &format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\n", k.replace(['\n', '\r'], " ").replace('\t', "\u{1f}"),cell(&r.qual), cell(&r.path), r.line, cell(&r.sig), r.dead as u8, r.callers);
    }
    // 作りかけを読まれないよう tmp に書いて rename (index の db と同じ流儀)。
    let tmp = path.with_extension(format!("{SNAP_EXT}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, s)?;
    std::fs::rename(&tmp, path)
}

/// 戻り値 = (snapshot, 採取時点の bake 時刻)。
pub(crate) fn read_snapshot(path: &Path) -> Option<(Snapshot, u32)> {
    let body = std::fs::read_to_string(path).ok()?;
    let mut lines = body.lines();
    if lines.next() != Some(SNAP_HEADER) {
        return None;
    }
    let baked_at: u32 = lines.next()?.strip_prefix(BAKED_PREFIX)?.parse().ok()?;
    let mut snap = Snapshot::new();
    for line in lines {
        // key 自体が tab 区切り 4 要素なので、key 内の tab は \x1f で退避してある。
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 7 {
            continue;
        }
        snap.insert(
            f[0].replace('\u{1f}', "\t"),
            SnapRow {
                qual: f[1].to_string(),
                path: f[2].to_string(),
                line: f[3].parse().unwrap_or(0),
                sig: f[4].to_string(),
                dead: f[5] == "1",
                callers: f[6].parse().unwrap_or(0),
            },
        );
    }
    Some((snap, baked_at))
}

/// 2 つの snapshot の差分。`sites` = 今の「名前 → 呼び出し位置」(壊れた参照の検出用)。
pub(crate) fn diff_snapshots(old: &Snapshot, new: &Snapshot, sites: &CallSites) -> Vec<Change> {
    let bare = |q: &str| q.rsplit("::").next().unwrap_or(q).to_string();
    let live_names: HashSet<String> = new.values().map(|r| bare(&r.qual)).collect();
    let n_defs = |s: &Snapshot| {
        let mut m: HashMap<String, usize> = HashMap::new();
        for r in s.values() {
            *m.entry(bare(&r.qual)).or_default() += 1;
        }
        m
    };
    let (old_n, new_n) = (n_defs(old), n_defs(new));
    // key が「一意 ↔ path 付き」で変わった定義 (同名が増えた / 減った) を同じ path で突き合わせる。
    // これをしないと、重複定義を足した瞬間に元の定義が「消えて別の物が生えた」に見え、確定を
    // 奪われたこと (callers → 0) が出ない。
    let new_by_path: HashMap<(&str, &str), &String> = new.iter().map(|(k, r)| ((key_base(k), r.path.as_str()), k)).collect();
    let mut matched: HashSet<&String> = HashSet::new();
    let mut out = Vec::new();
    for (k, o) in old {
        let hit = new.get_key_value(k).or_else(|| new_by_path.get(&(key_base(k), o.path.as_str())).and_then(|nk| new.get_key_value(*nk)));
        match hit {
            None => {
                // 同名の定義が別に残っていれば、呼び出しはそちらを指している可能性がある → 黙る
                // (壊れていないものを壊れたと言わない)。
                let name = bare(&o.qual);
                if live_names.contains(&name) {
                    continue;
                }
                if let Some(v) = sites.get(&name).filter(|v| !v.is_empty()) {
                    out.push(Change::Broken { qual: o.qual.clone(), at: v[0].clone(), remaining: v.len() as u32 });
                }
            }
            Some((nk, n)) => {
                matched.insert(nk);
                let at = (n.path.clone(), n.line);
                if o.sig != n.sig {
                    out.push(Change::Sig { qual: n.qual.clone(), at: at.clone(), old: o.sig.clone(), new: n.sig.clone(), callers: n.callers });
                }
                if !o.dead && n.dead {
                    out.push(Change::Dead { qual: n.qual.clone(), at: at.clone(), added: false });
                } else if o.dead && !n.dead {
                    out.push(Change::Revived { qual: n.qual.clone(), at: at.clone() });
                }
                if o.callers != n.callers {
                    let name = bare(&n.qual);
                    let dup = new_n.get(&name).copied().unwrap_or(0) > old_n.get(&name).copied().unwrap_or(0);
                    out.push(Change::Callers { qual: n.qual.clone(), at, old: o.callers, new: n.callers, dup });
                }
            }
        }
    }
    // 新しく足された定義は「dead で生まれた」時だけ報告する (足したのに誰も呼んでいない = 繋ぎ忘れ)。
    for (k, n) in new {
        if !matched.contains(k) && n.dead {
            out.push(Change::Dead { qual: n.qual.clone(), at: (n.path.clone(), n.line), added: true });
        }
    }
    let rank = |c: &Change| ["broken", "sig", "dead", "revived", "callers"].iter().position(|k| *k == c.kind()).unwrap_or(9);
    out.sort_by(|a, b| (rank(a), a.at()).cmp(&(rank(b), b.at())));
    out
}

/// key の path 部分 (衝突時に付く `\t@<path>[#n]`) を除いた部分。
fn key_base(k: &str) -> &str {
    k.split_once("\t@").map(|(b, _)| b).unwrap_or(k)
}

fn changes_dir(db: &str) -> PathBuf {
    PathBuf::from(format!("{db}.changes"))
}

/// token = 採取時刻 (ns、16 進)。辞書順 = 時刻順なので prune が名前の sort だけで済む。
fn new_token() -> String {
    let ns = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{ns:020x}")
}

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 古い snapshot を消す。cursor が指している物は残す (ただし放置された cursor は先に捨てる)。
fn prune_snapshots(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut snaps: Vec<String> = Vec::new();
    let mut pinned: HashSet<String> = HashSet::new();
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n.starts_with(CURSOR_PREFIX) {
            let idle = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok());
            if idle.is_some_and(|d| d > Duration::from_secs(CURSOR_TTL_DAYS * 86_400)) {
                let _ = std::fs::remove_file(e.path());
                continue;
            }
            if let Ok(t) = std::fs::read_to_string(e.path()) {
                pinned.insert(t.trim().to_string());
            }
        } else if let Some(t) = n.strip_suffix(&format!(".{SNAP_EXT}")) {
            snaps.push(t.to_string());
        }
    }
    snaps.sort();
    let drop = snaps.len().saturating_sub(CHANGES_KEEP);
    for t in &snaps[..drop] {
        if !pinned.contains(t) {
            let _ = std::fs::remove_file(dir.join(format!("{t}.{SNAP_EXT}")));
        }
    }
}

fn git(root: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").arg("-C").arg(root).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 使い捨ての場所 (OS の temp)。**cache の下には置かない** — `<db>.changes/.ignore` が親から効いて
/// 中身が丸ごと index 対象外になる。
fn scratch_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("kenning-changes-{tag}-{}-{}", std::process::id(), new_token()))
}

/// `src` を syn 層だけで使い捨て db に index して snapshot を採る。表示用 path は `src` → `shown` に読み替える。
/// 別 process で焼く (index の進捗出力と panic 捕捉を本体の経路に任せ、stdout を汚さない)。
fn snapshot_of_tree(src: &Path, shown: &str) -> Option<(Snapshot, CallSites)> {
    let db = scratch_dir("db");
    let exe = std::env::current_exe().ok()?;
    let ok = std::process::Command::new(exe)
        .arg("index").arg(src).arg(&db)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().ok()?.success();
    let r = if ok { Database::open_readonly(&db.to_string_lossy()).ok().map(|d| take_snapshot(&d)) } else { None };
    let _ = std::fs::remove_dir_all(&db);
    let _ = std::fs::remove_file(format!("{}.index.lock", db.display()));
    let (mut snap, mut sites) = r?;
    let from = abs_dir(&src.to_string_lossy());
    let fix = |p: &mut String| if let Some(rest) = p.strip_prefix(from.as_str()) { *p = format!("{shown}{rest}") };
    for r in snap.values_mut() {
        fix(&mut r.path);
    }
    for v in sites.values_mut() {
        v.iter_mut().for_each(|(p, _)| fix(p));
    }
    Some((snap, sites))
}

/// 過去の tree (git の commit / sinfo の snap) を `materialize` で使い捨て dir に書き出し、syn 層で
/// snapshot を採る。中身が不変な物だけを `key` で `<db>.changes/<sub>/` に cache (最新 CHANGES_KEEP 個)。
fn cached_tree_snapshot(db: &str, root: &str, sub: &str, key: &str, what: &str, materialize: impl FnOnce(&Path) -> bool) -> Result<Snapshot, String> {
    let dir = changes_dir(db).join(sub);
    let cached = dir.join(format!("{key}.{SNAP_EXT}"));
    if let Some((s, _)) = read_snapshot(&cached) {
        return Ok(s);
    }
    let tree = scratch_dir("tree");
    std::fs::create_dir_all(&tree).map_err(|e| e.to_string())?;
    let snap = if materialize(&tree) { snapshot_of_tree(&tree, root).map(|(s, _)| s) } else { None };
    let _ = std::fs::remove_dir_all(&tree);
    let snap = snap.ok_or_else(|| format!("{what} の tree を書き出せない / index できない"))?;
    if std::fs::create_dir_all(&dir).is_ok() && write_snapshot(&cached, &snap, 0).is_ok() {
        prune_by_mtime(&dir, CHANGES_KEEP);
    }
    Ok(snap)
}

/// `<ref>` 時点の tree の snapshot (commit hash で cache)。戻り値 = (snapshot, 解決した commit)。
fn git_ref_snapshot(db: &str, root: &str, gitref: &str) -> Result<(Snapshot, String), String> {
    let commit = git(root, &["rev-parse", "--verify", "--quiet", &format!("{gitref}^{{commit}}")])
        .ok_or_else(|| format!("\"{gitref}\" は token でも git ref でもない ({root} で rev-parse できない)"))?;
    let what = format!("{gitref} ({})", &commit[..commit.len().min(12)]);
    // git archive = tracked file だけ (ignore 済みの生成物は最初から入らない)。repo の状態には触らない。
    let snap = cached_tree_snapshot(db, root, "git", &commit, &what, |tree| {
        std::process::Command::new("sh")
            .arg("-c").arg("git -C \"$1\" archive --format=tar \"$2\" | tar -x -C \"$3\"")
            .arg("sh").arg(root).arg(&commit).arg(tree)
            .status().map(|s| s.success()).unwrap_or(false)
    })?;
    Ok((snap, commit))
}

/// sinfo の snap 1 件 (`sf snap list --json` から要る所だけ)。
#[derive(Debug, PartialEq)]
pub(crate) struct SinfoSnap {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) digest: String,
}

/// JSON 文字列値を `"key": "…"` の形で `from` 以降から 1 つ拾う (escape を解く)。依存を足さないための最小実装。
fn json_str_field(s: &str, key: &str, from: usize) -> Option<(String, usize)> {
    let pat = format!("\"{key}\": \"");
    let start = s[from..].find(&pat)? + from + pat.len();
    let mut out = String::new();
    let mut it = s[start..].char_indices();
    while let Some((i, c)) = it.next() {
        match c {
            '"' => return Some((out, start + i + 1)),
            '\\' => match it.next().map(|(_, e)| e) {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(e) => out.push(e),
                None => return None,
            },
            c => out.push(c),
        }
    }
    None
}

/// `sf snap list --json` の出力 (新しい順) から id / label / contentDigest を拾う。
pub(crate) fn parse_sinfo_snaps(json: &str) -> Vec<SinfoSnap> {
    let mut v = Vec::new();
    let mut pos = 0;
    while let Some((id, after_id)) = json_str_field(json, "id", pos) {
        let next = json[after_id..].find("\"id\": \"").map(|i| i + after_id).unwrap_or(json.len());
        let obj = &json[..next];
        let label = json_str_field(obj, "label", after_id).map(|x| x.0).unwrap_or_default();
        let digest = json_str_field(obj, "contentDigest", after_id).map(|x| x.0).unwrap_or_default();
        v.push(SinfoSnap { id, label, digest });
        pos = next;
    }
    v
}

/// `spec` = 空なら最新、そうでなければ id / id の前方一致 (短縮) / label の完全一致。
pub(crate) fn pick_sinfo_snap<'a>(snaps: &'a [SinfoSnap], spec: &str) -> Option<&'a SinfoSnap> {
    if spec.is_empty() {
        return snaps.first();
    }
    snaps.iter().find(|s| s.id == spec || s.label == spec).or_else(|| {
        let short = spec.strip_prefix("snap_").unwrap_or(spec);
        snaps.iter().find(|s| s.id.strip_prefix("snap_").is_some_and(|i| i.starts_with(short)))
    })
}

/// sinfo の snap (= build として整合する module の組) 時点の snapshot。`sf write snap` で書き出す
/// (project には触らない)。cache の鍵は contentDigest。戻り値 = (snapshot, 見出し用の名前)。
fn sinfo_snap_snapshot(db: &str, root: &str, spec: &str) -> Result<(Snapshot, String), String> {
    if !Path::new(root).join(".sinfo").is_dir() {
        return Err(format!("{root} は sinfo の project ではない (.sinfo が無い)"));
    }
    let out = std::process::Command::new("sf").args(["snap", "list", "--json", "-n", "200"]).current_dir(root).output()
        .map_err(|e| format!("sf を起動できない: {e}"))?;
    let snaps = parse_sinfo_snaps(&String::from_utf8_lossy(&out.stdout));
    let snap = pick_sinfo_snap(&snaps, spec).ok_or_else(|| {
        if spec.is_empty() { "snap が 1 つも無い".to_string() } else { format!("snap \"{spec}\" が見つからない (id / 短縮 id / label)") }
    })?;
    let short = snap.id.strip_prefix("snap_").unwrap_or(&snap.id);
    let name = format!("snap {} ({})", snap.label, &short[..short.len().min(8)]);
    let key = if snap.digest.is_empty() { snap.id.clone() } else { snap.digest.clone() };
    let s = cached_tree_snapshot(db, root, "sinfo", &key, &name, |tree| {
        std::process::Command::new("sf").args(["write", "snap", &snap.id, "--dest"]).arg(tree).current_dir(root)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .status().map(|st| st.success()).unwrap_or(false)
    })?;
    Ok((s, name))
}

/// 今の index の bake 時刻 (0 = 未 bake)。snapshot 同士の精度が揃っているかの判定に使う。
fn baked_at_of(db: &Database) -> u32 {
    db.get_table("meta")
        .and_then(|t| t.all().find().ok()?.into_iter().next())
        .and_then(|e| match db.get_table("meta")?.entity(e).get("baked_at") {
            Some(Value::Number(n)) => Some(n as u32), // num() は欠損を u32::MAX にするので使わない
            _ => None,
        })
        .unwrap_or(0)
}

/// `dir` 直下の snapshot を新しい順に `keep` 個だけ残す。
fn prune_by_mtime(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut v: Vec<(SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == SNAP_EXT))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    v.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, p) in v.into_iter().skip(keep) {
        let _ = std::fs::remove_file(p);
    }
}

/// `changes [--since <token|git ref|snap[:<id|label>]> | --cursor <name>] [--json] [--all]`
pub fn cmd_changes(args: &[String]) {
    let (mut since, mut cursor, mut as_json, mut all) = (None::<String>, None::<String>, false, false);
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--since" => { i += 1; since = args.get(i).cloned(); }
            "--cursor" => { i += 1; cursor = args.get(i).cloned(); }
            "--json" => as_json = true,
            "--all" => all = true,
            _ => rest.push(args[i].clone()),
        }
        i += 1;
    }
    if since.is_some() && cursor.is_some() {
        eprintln!("usage: kenning changes [--since <token> | --cursor <name>] [--json]  (どちらか一方)");
        std::process::exit(2);
    }
    if let Some(c) = cursor.as_deref().filter(|c| !valid_name(c)) {
        eprintln!("# cursor 名は英数と - _ . だけ: \"{c}\"");
        std::process::exit(2);
    }
    let o = parse_opts(&rest);
    let Some(db) = open_ro(&o.db) else { std::process::exit(2) };
    let dir = changes_dir(&o.db);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("# snapshot 置き場を作れない ({}): {e}", dir.display());
        std::process::exit(2);
    }
    // db を repo の中に置いた時 (`--db ./k.db`)、snapshot が text として index されると全 fn 名が
    // 「字句として出現」して dead 判定を殺す。walk (= rg と同じ規約) が尊重する .ignore で自分を隠す。
    let _ = std::fs::write(dir.join(".ignore"), "*\n");
    let cursor_file = cursor.as_ref().map(|c| dir.join(format!("{CURSOR_PREFIX}{c}")));
    let base_token = match (&since, &cursor_file) {
        (Some(t), _) => Some(t.clone()),
        (None, Some(f)) => std::fs::read_to_string(f).ok().map(|s| s.trim().to_string()),
        (None, None) => None,
    };
    let root = read_meta(&db).map(|m| m.0).unwrap_or_default();
    // 起点: token (その snapshot が在る) → sinfo の snap (`snap` / `snap:<id|label>`) → git ref の順に解く。
    let base = match &base_token {
        Some(t) if valid_name(t) && dir.join(format!("{t}.{SNAP_EXT}")).exists() => {
            read_snapshot(&dir.join(format!("{t}.{SNAP_EXT}"))).map(|(s, b)| Base::Stored(s, b)).or_else(|| missing_snapshot(t))
        }
        Some(t) if since.is_some() && !looks_like_token(t) => {
            if root.is_empty() {
                eprintln!("# この index は root を知らない (旧版) → ref を解けない。kenning index で焼き直しを");
                std::process::exit(2);
            }
            let r = match t.strip_prefix("snap:").or(if t == "snap" { Some("") } else { None }) {
                Some(spec) => sinfo_snap_snapshot(&o.db, &root, spec),
                None => git_ref_snapshot(&o.db, &root, t).map(|(s, c)| (s, format!("{t} ({})", &c[..c.len().min(12)]))),
            };
            match r {
                Ok((snap, label)) => Some(Base::Tree(snap, label)),
                Err(e) => {
                    eprintln!("# {e}");
                    std::process::exit(2);
                }
            }
        }
        Some(t) => missing_snapshot(t),
        None => None,
    };

    // 過去の tree は syn 層だけで焼く。今の db が bake 済みなら SCIP で確定した分だけ callers / dead が
    // 食い違うので、作業ツリー側も syn 層で焼き直して揃える (揃えないと偽の差分が出る)。状態は持たない。
    if let Some(Base::Tree(base, label)) = &base {
        let (now, sites) = if scip_stale_files(&db).is_some() {
            snapshot_of_tree(Path::new(&root), &root).unwrap_or_else(|| {
                eprintln!("# 作業ツリーを syn 層で index できない");
                std::process::exit(2);
            })
        } else {
            take_snapshot(&db)
        };
        let changes = diff_snapshots(base, &now, &sites);
        print_changes(&changes, label, None, o.limit, as_json, all);
        return;
    }
    let (now, sites) = take_snapshot(&db);
    let now_baked = baked_at_of(&db);
    let token = new_token();
    if let Err(e) = write_snapshot(&dir.join(format!("{token}.{SNAP_EXT}")), &now, now_baked) {
        eprintln!("# snapshot を書けない: {e}");
        std::process::exit(2);
    }
    if let Some(f) = &cursor_file {
        let _ = std::fs::write(f, &token);
    }
    prune_snapshots(&dir);

    let Some(Base::Stored(base, base_baked)) = base else {
        if as_json {
            println!("{{\"kind\":\"token\",\"token\":{},\"since\":null}}", json_str(&token));
        } else {
            println!("# baseline を作った ({} fn/method)。次回: kenning changes --since {token}", now.len());
            println!("# token: {token}");
        }
        return;
    };
    let mut changes = diff_snapshots(&base, &now, &sites);
    // 間に bake が入った (焼き直し / 自動 bake) 2 つは確定の精度が違う → 確定 callers 数の差は
    // コードの変化ではなく精度の差を含むので出さない (関数 1 つ変えていなくても一斉に動く)。
    if base_baked != now_baked {
        changes.retain(|c| !matches!(c, Change::Callers { .. }));
        if !as_json {
            println!("# 起点から今までに bake が入った (精度が変わった) → callers の増減は省略。dead / revived も精度差を含み得る");
        }
    }
    print_changes(&changes, base_token.as_deref().unwrap_or(""), Some(&token), o.limit, as_json, all);
}

/// 起点。Stored = 自分で採った snapshot (採取時の bake 時刻付き)、Tree = 過去の tree を syn 層で焼いた物。
enum Base {
    Stored(Snapshot, u32),
    Tree(Snapshot, String),
}

/// 起点の snapshot が読めない時。黙って baseline 扱いにしない (「差分なし」と読まれてしまう)。
fn missing_snapshot(t: &str) -> Option<Base> {
    eprintln!("# ⚠ snapshot {t} が無い (prune 済み・別 db の token・旧形式)。今の状態を新しい baseline にした");
    None
}

fn is_default_visible(c: &Change) -> bool {
    !matches!(c, Change::Callers { new, .. } if *new != 0)
}

/// token 形 (new_token の出力 = 20 桁の 16 進)。git の短縮 hash (7〜12 桁) とは長さで分かれる。
fn looks_like_token(s: &str) -> bool {
    s.len() == 20 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// `since` = 見出しに出す起点。`token` = 次回の起点 (git ref 起点は状態を持たないので None)。
/// `all` = callers (呼び元数の増減) の行も全部出す。既定は **0 になった物だけ** — 増減の大半は手を打つ物
/// ではなく、実測 (kenning 自身の 1 commit) で 19 行が broken / sig / dead を埋もれさせた。一方 0 になった物は
/// 重複定義に呼び出しを奪われた等の兆候で、名前が字句として残るので dead にも出ない (自分で書いた
/// `json_str` の重複がこれでしか見つからなかった)。JSON は機械が絞るので常に全件。
fn print_changes(changes: &[Change], since: &str, token: Option<&str>, limit: usize, as_json: bool, all: bool) {
    if as_json {
        for c in changes {
            println!("{}", c.json());
        }
        let t = token.map(json_str).unwrap_or_else(|| "null".into());
        println!("{{\"kind\":\"token\",\"token\":{t},\"since\":{}}}", json_str(since));
        return;
    }
    let count = |k: &str| changes.iter().filter(|c| c.kind() == k).count();
    println!(
        "# changes since {since}: broken {} / sig {} / dead {} / revived {} / callers {}",
        count("broken"), count("sig"), count("dead"), count("revived"), count("callers")
    );
    let shown: Vec<&Change> = changes.iter().filter(|c| all || is_default_visible(c)).collect();
    for c in shown.iter().take(limit) {
        let (p, l) = c.at();
        println!("{p}:{l}\t{}\t{}", c.kind(), c.detail());
    }
    if shown.len() > limit {
        println!("{}", omitted(shown.len(), limit));
    }
    let hidden = changes.len() - shown.len();
    if hidden > 0 {
        println!("# callers の増減 {hidden} 件は省略 (--all で表示)");
    }
    if let Some(t) = token {
        println!("# token: {t}");
    }
}
