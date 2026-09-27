//! `changes` — 前回の snapshot からの**意味的な差分** (壊れた参照 / シグネチャ変更 / 新しく dead /
//! 復活 / 呼び元数の増減)。読み手を選ばない: 既定は `path:line<TAB>種類<TAB>詳細` (人間・Claude・
//! エディタ)、`--json` で NDJSON (CI・スクリプト)。
//!
//! snapshot は db の隣 `<db>.changes/<token>.tsv` に置く (db 本体の schema は触らない = INDEX_VER 不変、
//! `cache prune` は prefix で一緒に回収する)。key は eid ではなく `crate / module / kind / Container::name`
//! なので、full 再 index を挟んでも比べられる。対象は fn / method だけ (call edge を持つのはこれだけ)。
//!
//! 起点の渡し方は 2 つ: `--since <token>` (呼び手が token を持つ) と `--cursor <name>` (名前付きで
//! kenning 側が token を進める = hook のように状態を持てない呼び手向け)。どちらも無ければ baseline を
//! 作って token だけ返す。

use super::*;

/// 消さずに残す snapshot の数 (cursor が指している物は別枠で必ず残す)。
pub(crate) const CHANGES_KEEP: usize = 8;
/// この日数 使われていない cursor は捨てる (hook は session ごとに cursor を作るので、終わった session の
/// cursor が snapshot を pin したまま溜まり続けないように)。
pub(crate) const CURSOR_TTL_DAYS: u64 = 7;
const SNAP_EXT: &str = "tsv";
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

/// 差分 1 件。`kind` の並び = 深刻な順 (出力もこの順)。
#[derive(Debug, PartialEq)]
pub(crate) enum Change {
    /// 定義が消えたのに、その名前の呼び出しが残っている (at = 残っている呼び出しの 1 つ)。
    Broken { qual: String, at: (String, u32), remaining: u32 },
    Sig { qual: String, at: (String, u32), old: String, new: String, callers: u32 },
    /// `added` = 今回足された定義 (足したのに誰も呼んでいない = 繋ぎ忘れ)。
    Dead { qual: String, at: (String, u32), added: bool },
    Revived { qual: String, at: (String, u32) },
    Callers { qual: String, at: (String, u32), old: u32, new: u32 },
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
            Change::Callers { qual, old, new, .. } => format!("{qual}: callers {old} → {new}"),
        }
    }
    fn json(&self) -> String {
        let (p, l) = self.at();
        let mut s = format!("{{\"kind\":\"{}\",\"path\":{},\"line\":{l},\"symbol\":{}", self.kind(), json_str(p), json_str(self.qual()));
        match self {
            Change::Broken { remaining, .. } => s += &format!(",\"remaining\":{remaining}"),
            Change::Sig { old, new, callers, .. } => s += &format!(",\"old\":{},\"new\":{},\"callers\":{callers}", json_str(old), json_str(new)),
            Change::Callers { old, new, .. } => s += &format!(",\"old\":{old},\"new\":{new}"),
            Change::Dead { added, .. } => s += &format!(",\"added\":{added}"),
            Change::Revived { .. } => {}
        }
        s + "}"
    }
}

pub(crate) fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// TSV の 1 セルに入れられる形 (tab / 改行を潰す)。
fn cell(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// 今の index から snapshot を採る。dead は `search reachable:0` と同じ判定 (字句照合込み = 安全側)。
/// 同時に「名前 → 呼び出し位置」も返す (消えた定義の呼び出しが残っているかを見るため)。
pub(crate) fn take_snapshot(db: &Database) -> (Snapshot, HashMap<String, Vec<(String, u32)>>) {
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
    let mut sites: HashMap<String, Vec<(String, u32)>> = HashMap::new();
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

    let mut snap = Snapshot::new();
    for kind in [K_FN, K_METHOD] {
        for e in sym_t.where_eq("kind", kind).find().unwrap_or_default() {
            let er = sym_t.entity(e);
            let qual = sym_qual(&sym_t, e);
            let base = format!("{}\t{}\t{}\t{qual}", txt(er.get("crate_")), txt(er.get("module")), kind_name(kind));
            let row = SnapRow {
                qual,
                path: paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(),
                line: num(er.get("line")),
                sig: txt(er.get("sig")),
                dead: dead.contains(&e),
                callers: callers.get(&e).copied().unwrap_or(0),
            };
            insert_unique(&mut snap, base, row);
        }
    }
    (snap, sites)
}

/// 同じ key の定義 (cfg 分岐の双子など) は `#2`, `#3` を振って両方残す。path:line 順に振り直すので、
/// 挿入順 (eid 順) に依らず同じ repo からは同じ key が出る。
fn insert_unique(snap: &mut Snapshot, base: String, row: SnapRow) {
    let mut group: Vec<SnapRow> = vec![row];
    let mut n = 1;
    loop {
        let k = if n == 1 { base.clone() } else { format!("{base}#{n}") };
        match snap.remove(&k) {
            Some(r) => group.push(r),
            None => break,
        }
        n += 1;
    }
    group.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    for (i, r) in group.into_iter().enumerate() {
        let k = if i == 0 { base.clone() } else { format!("{base}#{}", i + 1) };
        snap.insert(k, r);
    }
}

pub(crate) fn write_snapshot(path: &Path, snap: &Snapshot) -> std::io::Result<()> {
    let mut s = String::new();
    for (k, r) in snap {
        s += &format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\n", k.replace(['\n', '\r'], " ").replace('\t', "\u{1f}"),cell(&r.qual), cell(&r.path), r.line, cell(&r.sig), r.dead as u8, r.callers);
    }
    // 作りかけを読まれないよう tmp に書いて rename (index の db と同じ流儀)。
    let tmp = path.with_extension(format!("{SNAP_EXT}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, s)?;
    std::fs::rename(&tmp, path)
}

pub(crate) fn read_snapshot(path: &Path) -> Option<Snapshot> {
    let body = std::fs::read_to_string(path).ok()?;
    let mut snap = Snapshot::new();
    for line in body.lines() {
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
    Some(snap)
}

/// 2 つの snapshot の差分。`sites` = 今の「名前 → 呼び出し位置」(壊れた参照の検出用)。
pub(crate) fn diff_snapshots(old: &Snapshot, new: &Snapshot, sites: &HashMap<String, Vec<(String, u32)>>) -> Vec<Change> {
    let bare = |q: &str| q.rsplit("::").next().unwrap_or(q).to_string();
    let live_names: HashSet<String> = new.values().map(|r| bare(&r.qual)).collect();
    let mut out = Vec::new();
    for (k, o) in old {
        match new.get(k) {
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
            Some(n) => {
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
                    out.push(Change::Callers { qual: n.qual.clone(), at, old: o.callers, new: n.callers });
                }
            }
        }
    }
    // 新しく足された定義は「dead で生まれた」時だけ報告する (足したのに誰も呼んでいない = 繋ぎ忘れ)。
    for (k, n) in new {
        if !old.contains_key(k) && n.dead {
            out.push(Change::Dead { qual: n.qual.clone(), at: (n.path.clone(), n.line), added: true });
        }
    }
    let rank = |c: &Change| ["broken", "sig", "dead", "revived", "callers"].iter().position(|k| *k == c.kind()).unwrap_or(9);
    out.sort_by(|a, b| (rank(a), a.at()).cmp(&(rank(b), b.at())));
    out
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

/// `changes [--since <token> | --cursor <name>] [--json]`
pub fn cmd_changes(args: &[String]) {
    let (mut since, mut cursor, mut as_json) = (None::<String>, None::<String>, false);
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--since" => { i += 1; since = args.get(i).cloned(); }
            "--cursor" => { i += 1; cursor = args.get(i).cloned(); }
            "--json" => as_json = true,
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
    let base = match &base_token {
        Some(t) if valid_name(t) => match read_snapshot(&dir.join(format!("{t}.{SNAP_EXT}"))) {
            Some(s) => Some(s),
            None => {
                // 消えた token は黙って baseline 扱いにしない (「差分なし」と読まれてしまう)。
                eprintln!("# ⚠ snapshot {t} が無い (prune 済みか別 db の token)。今の状態を新しい baseline にした");
                None
            }
        },
        Some(t) => {
            eprintln!("# token の形式が違う: \"{t}\"");
            std::process::exit(2);
        }
        None => None,
    };

    let (now, sites) = take_snapshot(&db);
    let token = new_token();
    if let Err(e) = write_snapshot(&dir.join(format!("{token}.{SNAP_EXT}")), &now) {
        eprintln!("# snapshot を書けない: {e}");
        std::process::exit(2);
    }
    if let Some(f) = &cursor_file {
        let _ = std::fs::write(f, &token);
    }
    prune_snapshots(&dir);

    let Some(base) = base else {
        if as_json {
            println!("{{\"kind\":\"token\",\"token\":{},\"since\":null}}", json_str(&token));
        } else {
            println!("# baseline を作った ({} fn/method)。次回: kenning changes --since {token}", now.len());
            println!("# token: {token}");
        }
        return;
    };
    let changes = diff_snapshots(&base, &now, &sites);
    let since_t = base_token.unwrap_or_default();
    if as_json {
        for c in &changes {
            println!("{}", c.json());
        }
        println!("{{\"kind\":\"token\",\"token\":{},\"since\":{}}}", json_str(&token), json_str(&since_t));
        return;
    }
    let count = |k: &str| changes.iter().filter(|c| c.kind() == k).count();
    println!(
        "# changes since {since_t}: broken {} / sig {} / dead {} / revived {} / callers {}",
        count("broken"), count("sig"), count("dead"), count("revived"), count("callers")
    );
    for c in changes.iter().take(o.limit) {
        let (p, l) = c.at();
        println!("{p}:{l}\t{}\t{}", c.kind(), c.detail());
    }
    if changes.len() > o.limit {
        println!("… (+{} 件省略、--limit {} で全部)", changes.len() - o.limit, changes.len());
    }
    println!("# token: {token}");
}
