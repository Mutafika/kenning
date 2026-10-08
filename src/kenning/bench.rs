//! ベンチスイート (quality / agent / beyond / text / micro)。

use super::*;

// ═══════════════════ bench suite (再現可能な計測 — 逸話でなく分布を出す) ═══════════════════
//
// `bench [quality|agent|micro|all]` — 出力は markdown (RESULTS.md にリダイレクトする前提)。
// quality: ランダム N シンボルで grep 相当ヒット vs 確実/候補 callers → ノイズ除去率の分布。
// agent:   被呼数上位の固定質問を grep 経路 (rg 出力 + 文脈 Read のバイトモデル) vs
//          kenning 経路 (実バイナリ実行の実出力バイト) で再生 → token 削減比の分布。
// micro:   コマンド latency (warm)。
// 乱数は固定 seed の LCG (依存なし・決定的)。コーパス取得は bench/corpus.sh (tag 固定)。

/// 決定的な擬似乱数 (splitmix64)。ベンチのシンボル抽出専用。
pub(crate) struct Lcg(pub(crate) u64);
impl Lcg {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
}

/// index 済み全ファイルを disk から読む (path, src)。ベンチの grep 相当スキャンと Read モデルに使う。
pub(crate) fn bench_files(file_t: &Table) -> Vec<(String, String)> {
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
pub(crate) fn grep_call_hit_lines(src: &str, name: &str) -> Vec<usize> {
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

pub(crate) fn median_u(mut v: Vec<usize>) -> usize {
    if v.is_empty() {
        return 0;
    }
    v.sort();
    v[v.len() / 2]
}

pub(crate) fn median_f(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// 呼ばれている fn/method の distinct 名 (決定的順序)。quality の母集団 & agent の質問源。
pub(crate) fn bench_target_names(sym_t: &Table, call_t: &Table) -> Vec<String> {
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
pub(crate) fn classify_calls(sym_t: &Table, call_t: &Table, name: &str) -> (usize, usize, usize) {
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
pub(crate) fn bench_quality(sym_t: &Table, call_t: &Table, files: &[(String, String)], n: usize, seed: u64) {
    let mut pool = bench_target_names(sym_t, call_t);
    let mut rng = Lcg(seed);
    // Fisher–Yates で先頭 n 件を決定的に抽出
    for i in 0..pool.len().saturating_sub(1).min(n) {
        let j = i + (rng.next() as usize) % (pool.len() - i);
        pool.swap(i, j);
    }
    let sample: Vec<String> = pool.into_iter().take(n).collect();

    println!("### quality — grep-style hits vs precise callers (n={}, seed={})\n", sample.len(), seed);
    println!("grep-style = every `\\bNAME\\s*\\(` hit (mixes in defs / comments / strings / same-named items of other types).");
    println!("confirmed = reverse lookup of callee_sym (never wrong) / unresolved candidates = unresolved name matches (need a look).\n");

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

    println!("| symbol | grep hits | confirmed | candidates | gap vs grep (≈noise) |");
    println!("|---|---|---|---|---|");
    for (g, name, sure, cand, nz) in rows.iter().take(10) {
        println!("| {name} | {g} | {sure} | {cand} | {nz} |");
    }
    println!("| … | (top 10 only) | | | |\n");
    println!(
        "**median**: grep {} lines → confirmed {} + candidates {} = {} lines to check, noise {:.0}%\n",
        median_u(greps),
        median_u(sures.clone()),
        median_u(boths.iter().zip(&sures).map(|(b, s)| b - s).collect()),
        median_u(boths),
        median_f(noise) * 100.0
    );
}

/// 「定義が一意」な被呼シンボルを確実 caller 数降順で (agent/beyond の質問選定、DRY)。
pub(crate) fn unique_def_ranked(sym_t: &Table, call_t: &Table) -> Vec<(usize, String)> {
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
pub(crate) fn grep_route_cost(files: &[(String, String)], hit_fn: &dyn Fn(&str) -> Vec<usize>) -> (usize, usize, usize) {
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
pub(crate) fn word_in(line: &str, word: &str) -> bool {
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
pub(crate) fn line_hits(src: &str, pred: &dyn Fn(&str) -> bool) -> Vec<usize> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| pred(l))
        .map(|(i, _)| i + 1)
        .collect()
}

/// kenning を別プロセスで実行して stdout バイト数を返す (実測、鮮度チェック抜き)。
pub(crate) fn cs_run_bytes(exe: &std::path::Path, db_path: &str, args: &[&str]) -> usize {
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
pub(crate) fn bench_beyond(db_path: &str, db: &Database, sym_t: &Table, call_t: &Table, files: &[(String, String)]) {
    let exe = std::env::current_exe().unwrap();
    println!("### beyond-search — graph / structure queries (grep-path model vs real output)\n");
    println!("The grep path uses the same optimistic model as agent (= a lower bound). For impact it is a manual BFS");
    println!("repeating grep+Read per visited symbol (mimics a real agent's recursive search).\n");

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
        println!("**impact** (transitive callers, top 5 questions):\n");
        println!("| question | affected syms | grep bytes | grep calls | cs bytes | ratio |");
        println!("|---|---|---|---|---|---|");
        for (n, v, gb, gc, cb, r) in &rows {
            println!("| impact {n} | {v} | {gb} | {gc} | {cb} | {r:.0}x |");
        }
        println!(
            "\nmedian: **{:.0}x**, tool calls {} → 1\n",
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
            println!("**impls** (trait → implementing types, most impls first):\n");
            println!("| question | impls | grep bytes | grep calls | ratio |");
            println!("|---|---|---|---|---|");
            for (t, n, gb, gc, r) in &rows {
                println!("| impls {t} | {n} | {gb} | {gc} | {r:.1}x |");
            }
            println!("\nmedian: **{:.1}x**\n", median_f(rows.iter().map(|r| r.4).collect()));
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
    println!("**outline** (file structure, 5 largest files — the alternative is reading the whole file):\n");
    println!("| file | Read bytes | outline bytes | ratio |");
    println!("|---|---|---|---|");
    for (p, fb, ob, r) in &rows {
        let short = p.rsplit('/').next().unwrap_or(p);
        println!("| {short} | {fb} | {ob} | {r:.0}x |");
    }
    println!("\nmedian: **{:.0}x**\n", median_f(rows.iter().map(|r| r.3).collect()));

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
        println!("**def** (definition + sig + doc, 10 most-called — the alternative is `rg \"fn NAME\"` + reading around it):\n");
        println!(
            "median: grep {} B / {} calls → def {} B / 1 call = **{:.1}x**\n",
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
        "**faceted** (`kind:method vis:pub test:0`): {} results {:?} — grep cannot express this (no comparison; a capability gap)\n",
        n,
        t.elapsed()
    );
}

/// ④エージェント級: 「誰が呼ぶ？」固定質問を grep 経路 vs kenning 経路で再生し、
/// tool 出力バイト数 (≈token) と呼び出し回数を比較。
/// grep 経路モデル: rg 出力 (`path:line:linetext`) + ヒットのある各ファイルを 40 行 Read
/// (ファイルごと 1 回 = 楽観的な agent。実際は再 grep や全 Read も多いので保守的な下限)。
pub(crate) fn bench_agent(db_path: &str, root: &str, sym_t: &Table, call_t: &Table, files: &[(String, String)], nq: usize) {
    // 質問 = 「定義が一意」なシンボルの確実 caller 数上位 nq。一意に限るのは公平性のため:
    // `new` みたいな多重定義名は実際の agent も `Type::new` で grep するので、`\bname\(` モデルが
    // grep 側に不当に不利になる。一意名なら `\bname\(` はまさに agent が打つ手 = 対等な比較。
    let questions: Vec<String> = unique_def_ranked(sym_t, call_t).into_iter().take(nq).map(|(_, n)| n).collect();

    println!("### agent — \"who calls it?\": tool output bytes over {} questions\n", questions.len());
    println!("Questions = most-called symbols with a unique definition (fair to grep: `\\bname\\(` targets them exactly).");
    println!("grep path = rg output + a 40-line Read per hit file (optimistic model = lower bound).");
    println!("kenning path = the real output of `callers <name>` (measured as a separate process).\n");
    println!("| question | grep bytes | grep calls | cs bytes | cs calls | ratio |");
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
        "\n**median**: ratio **{:.1}x**, grep-path tool calls {} → 1\n",
        median_f(ratios),
        median_u(gcalls)
    );
    if has_rg {
        println!(
            "**single-run wall-clock median**: rg {:.1}ms vs kenning {:.1}ms (both include process start.\nkenning without the freshness check; by default add ~10ms of stat-walk. Speed is even — the difference is precision and bytes)\n",
            median_f(rg_ms),
            median_f(cs_ms.clone())
        );
    }
    if has_sg && !sg_rows.is_empty() {
        println!("### agent — vs ast-grep (structural search tool, same questions)\n");
        println!("ast-grep matches tree-sitter structure: **0** noise from defs / comments / strings (one step more precise than grep).");
        println!("But the user has to list the 3 call shapes (`name()` / `$R.name()` / `$P::name()`), and");
        println!("**there is no name resolution** — `$R.name()` matches same-named methods on every type, so it cannot say");
        println!("which definition is called (= kenning's \"unresolved candidates\" granularity). It walks files, so it slows down with repo size.\n");
        println!("| question | ast-grep matches | bytes | ms (3 patterns) | cs confirmed+candidates | cs bytes | cs ms |");
        println!("|---|---|---|---|---|---|---|");
        let n = sg_rows.len().min(cs_ms.len());
        for (i, (q, hits, bytes, ms, sure, cand, csb)) in sg_rows.iter().take(n).enumerate() {
            println!(
                "| callers {q} | {hits} | {bytes} | {ms:.0} | {sure}+{cand} | {csb} | {:.1} |",
                cs_ms[i]
            );
        }
        println!(
            "\n**median**: ast-grep {:.0}ms / {}B vs kenning {:.1}ms / {}B — both find the same structural matches, but\nast-grep cannot confirm \"which definition\", and has no impact / path / faceted\n",
            median_f(sg_rows.iter().map(|r| r.3).collect()),
            median_u(sg_rows.iter().map(|r| r.2).collect()),
            median_f(cs_ms),
            median_u(sg_rows.iter().map(|r| r.6).collect())
        );
    }
}

/// ①マイクロ: コマンドの warm latency (generic — どの repo の db でも動く)。
/// `text` の件数行 `# N 件 / M files …` から N を読む (bench 用)。
/// 0 件の時の出力は「index 済みファイルに無い」行なので数字は取れず 0 になる。
pub(crate) fn text_hit_count(stdout: &str) -> usize {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("# ").and_then(|r| r.split(' ').next()).and_then(|n| n.parse::<usize>().ok()))
        .unwrap_or(0)
}

/// ⑤全文検索: 同じ語を `rg -i -n` と `kenning text` に投げ、ヒット数の一致・wall・出力バイトを比較。
/// 他スイートと違い「grep の置き換えになるか」だけを測る — 圧縮比ではなく **同じ答えを同じ速さで
/// 返すか** が問い (kenning 側は文脈注釈の分バイトは増える。減らすのが目的ではない)。
/// 質問 = corpus 内の高頻度 token (長さ 6 以上、決定的順序)。
pub(crate) fn bench_text(db_path: &str, root: &str, files: &[(String, String)], nq: usize) {
    let mut freq: HashMap<&str, usize> = HashMap::new();
    for (_, src) in files {
        for tok in src.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
            if tok.len() >= 6 && !tok.chars().all(|c| c.is_ascii_digit()) {
                *freq.entry(tok).or_default() += 1;
            }
        }
    }
    let mut ranked: Vec<(usize, &str)> = freq.into_iter().map(|(t, c)| (c, t)).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1))); // 頻度降順 → 語昇順 (決定的)
    let terms: Vec<&str> = ranked.into_iter().take(nq).map(|(_, t)| t).collect();

    println!("### text — full-text search vs rg (same terms, same repo)\n");
    println!("Question = \"where is this term?\". rg path = `rg -i -n <term>` (-i because kenning text ignores case).");
    println!("kenning path = the real output of `text <term> --limit 100000`. The main metric is **matching hit counts** —");
    println!("kenning prints more bytes (each line carries its function name / heading path). That is the payload.\n");
    println!("| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | match |");
    println!("|---|---|---|---|---|---|---|---|");

    let exe = std::env::current_exe().unwrap();
    let has_rg = std::process::Command::new("rg").arg("--version").output().is_ok();
    // 差は 2 方向あり、原因が違う (実測: ripgrep corpus で両方出た)。
    //   少ない = 生成 lock (`Cargo.lock`) / >1MiB / binary — kenning が索引しないと決めた分
    //   多い   = NUL を含む file。rg は binary 判定で途中で打ち切るが kenning は最後まで読む
    let (mut rg_ms, mut cs_ms, mut agree, mut fewer, mut more) = (Vec::new(), Vec::new(), 0usize, 0usize, 0usize);
    for term in &terms {
        let (mut rg_hits, mut rg_bytes, mut rgms) = (0usize, 0usize, 0.0);
        if has_rg {
            let t0 = Instant::now();
            let out = std::process::Command::new("rg").args(["-i", "-n", term, root]).output();
            rgms = t0.elapsed().as_secs_f64() * 1000.0;
            if let Ok(o) = out {
                rg_bytes = o.stdout.len();
                rg_hits = o.stdout.split(|&b| b == b'\n').filter(|l| !l.is_empty()).count();
            }
            rg_ms.push(rgms);
        }
        let t0 = Instant::now();
        let out = std::process::Command::new(&exe)
            .args(["text", term, "--limit", "100000", "--db", db_path])
            .env("KENNING_NO_STALE", "1")
            .output();
        let csms = t0.elapsed().as_secs_f64() * 1000.0;
        cs_ms.push(csms);
        let stdout = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        let cs_bytes = stdout.len();
        let cs_hits = text_hit_count(&stdout);
        let same = cs_hits == rg_hits;
        match cs_hits.cmp(&rg_hits) {
            std::cmp::Ordering::Equal => agree += 1,
            std::cmp::Ordering::Less => fewer += 1,
            std::cmp::Ordering::Greater => more += 1,
        }
        println!(
            "| {term} | {rg_hits} | {rgms:.0} | {rg_bytes} | {cs_hits} | {csms:.0} | {cs_bytes} | {} |",
            if same { "=" } else { "≠" }
        );
    }
    let mut diff = String::new();
    if fewer > 0 {
        diff.push_str(&format!(" {fewer} fewer = generated lock (`Cargo.lock`) / >1MiB / binary files, not indexed."));
    }
    if more > 0 {
        diff.push_str(&format!(" {more} more = files containing NUL (rg stops, treating them as binary; kenning reads to the end)."));
    }
    println!(
        "\n**match**: same hit count on {}/{} terms. {}**wall median**: rg {:.1}ms vs text {:.1}ms",
        agree,
        terms.len(),
        if diff.is_empty() { String::new() } else { format!("Differences:{diff} ") },
        median_f(rg_ms),
        median_f(cs_ms)
    );
    println!("(both include process start; text's wall includes its `#` count line and context annotations)\n");
}

pub(crate) fn bench_micro(db_path: &str, sym_t: &Table, call_t: &Table, files: &[(String, String)]) {
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
    timed("pub async fn non-test", || {
        sym_t.where_eq("kind", K_FN).where_eq("vis", V_PUB)
            .where_eq("is_async", 1u32).where_eq("is_test", 0u32).count().unwrap()
    });
    // 点クエリは被呼最多シンボルで (どの repo でも存在する)
    if let Some(name) = bench_target_names(sym_t, call_t)
        .into_iter()
        .max_by_key(|n| call_t.where_eq("callee", n.as_str()).count().unwrap())
    {
        timed(&format!("def {name}"), || sym_t.where_eq("name", name.as_str()).count().unwrap());
        timed(&format!("callers {name} (name match)"), || {
            call_t.where_eq("callee", name.as_str()).count().unwrap()
        });
        if let Some(d) = sym_t.where_eq("name", name.as_str()).find().unwrap().first().copied() {
            timed(&format!("callers {name} (confirmed reverse lookup)"), || {
                call_t.where_eq("callee_sym", Value::Ref(d)).count().unwrap()
            });
        }
    }
    println!("```\n");
}

/// `bench [quality|agent|beyond|text|micro|infer|all] [--db P] [--n N] [--seed S]` — markdown を stdout へ。
pub fn cmd_bench(args: &[String]) {
    let mut sub = "all".to_string();
    let mut n = 100usize;
    let mut nq = 20usize;
    let mut seed = 42u64;
    let mut rest: Vec<String> = Vec::new();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "quality" | "agent" | "beyond" | "text" | "micro" | "infer" | "all" => sub = a.clone(),
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
    let rs = ResolveStats::of(&call_t);
    // db の実 disk と vocab 索引の分 (#15 の回帰防止: entity 数比例の予約に戻ると索引が 2/3 を占める)。
    let mb = |p: &Path| real_bytes(p) as f64 / 1_048_576.0;
    println!(
        "corpus `{}` — {} files / {} call-sites ({} in repo) / in-repo confirmed rate {:.1}% / {} / db on disk {:.1} MB (vocab index {:.1} MB)\n",
        root,
        files.len(),
        rs.total,
        rs.local(),
        rs.local_pct(),
        if baked { "**baked (SCIP)**" } else { "syn-only (not baked)" },
        mb(Path::new(&o.db)),
        mb(&Path::new(&o.db).join(VOCAB_INDEX_SEG))
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
    if sub == "text" || sub == "all" {
        bench_text(&o.db, &root, &files, nq);
    }
    if sub == "micro" || sub == "all" {
        bench_micro(&o.db, &sym_t, &call_t, &files);
    }
    // infer は bake 済み db を正解に使うので、明示した時だけ (all には入れない: syn 層の index を 1 本焼く)
    if sub == "infer" {
        if baked {
            bench_infer(&root, &o.db, &db);
        } else {
            println!("infer: this index is not baked — no ground truth (rust-analyzer) to measure against. Run after `kenning bake`");
        }
    }
}

/// call 1 行の突き合わせ鍵 (path, line, callee 名) → 解決先の定義 (path, line) と res。
type CallKey = (String, u32, String);
type CallRows = HashMap<CallKey, Vec<(Option<(String, u32)>, u32)>>;
fn call_rows(db: &Database) -> CallRows {
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);
    let mut m = CallRows::new();
    for c in call_t.all().find().unwrap_or_default() {
        let er = call_t.entity(c);
        let key = (paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(), num(er.get("line")), txt(er.get("callee")));
        let target = match er.get("callee_sym") {
            Some(Value::Ref(t)) => {
                let se = sym_t.entity(t);
                Some((paths.get(&ref_of(se.get("file"))).cloned().unwrap_or_default(), num(se.get("line"))))
            }
            _ => None,
        };
        m.entry(key).or_default().push((target, num(er.get("res"))));
    }
    m
}

/// SCIP symbol の末尾の名前。symbol は `<scheme> <manager> <package> <version> <descriptors>` で、
/// version に `.` を含むので descriptors だけを見る (`summary/impl#[SummaryBuilder]new().` → `new`、
/// `impl#[`Parser<'a>`]bump().` → `bump`、`print_stats().` → `print_stats`)。
pub(crate) fn scip_name(sym: &str) -> &str {
    let desc = sym.splitn(5, ' ').nth(4).unwrap_or(sym).trim_end_matches(['.', '#']);
    let last = desc.rsplit(['/', '#', ']']).next().unwrap_or(desc);
    last.split('(').next().unwrap_or(last).trim_matches('`')
}

/// SCIP symbol の package 欄 (3 つ目)。repo の crate なら「定義位置が読めない = 判定不能」、外なら「repo の外」。
pub(crate) fn scip_package(sym: &str) -> &str {
    sym.split(' ').nth(2).unwrap_or("")
}

/// ⑥型推定の答え合わせ: 同じ repo を syn 層だけで焼き直し、syn 層が**確定**した call を、同じ行で
/// rust-analyzer (.scip) が同じ名前に付けた symbol の定義位置と突き合わせる。一致 = 正しい確定、
/// 不一致 = 誤確定 (「確実 = 誤りなし」を破っている)、RA が沈黙 / 同じ行に同名が複数 = 判定不能。
/// bake 後に内容が変わった file は正解が無いので数えない (bake 直後ほど母数が多い)。
fn bench_infer(root: &str, db_path: &str, baked_db: &Database) {
    let Some(scip_path) = scip_sidecar_of(db_path) else {
        println!("infer: no .scip — run after `kenning bake`");
        return;
    };
    let tmp = std::env::temp_dir().join(format!("kenning-bench-infer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let exe = std::env::current_exe().unwrap();
    let ok = std::process::Command::new(exe).arg("index").arg(root).arg(&tmp)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().is_ok_and(|s| s.success());
    let syn_db = if ok { Database::open_readonly(&tmp.to_string_lossy()).ok() } else { None };
    let Some(syn_db) = syn_db else {
        println!("infer: cannot build the syn-layer index");
        return;
    };
    // 正解は .scip から直接読む (bake 済み db は増分 update で syn 層の答えが混ざるので使わない)。
    // Scip::load は bake 後に内容が変わった file を捨てる = 残るのは正解として信じてよい file だけ。
    let scip = Scip::load(&scip_path, root);
    // symbol → 定義位置 (rel, 1-indexed)。None = 同じ symbol が複数の定義を持つ (test / example の target 間で
    // 同名の関数が衝突する) → RA の答えは当てにならないので正解に使わない (index 側も確定に使わない規則)。
    let mut def_at: HashMap<&str, Option<(String, u32)>> = HashMap::new();
    let mut on_line: HashMap<(String, u32), Vec<&str>> = HashMap::new(); // (rel, 1-indexed) → 参照 symbol
    for o in &scip.occ {
        if o.roles & 1 == 1 {
            let at = (o.rel_path.clone(), o.line0 + 1);
            def_at.entry(o.symbol.as_str()).and_modify(|d| if d.as_ref() != Some(&at) { *d = None }).or_insert(Some(at));
        } else if !o.symbol.starts_with("local ") {
            on_line.entry((o.rel_path.clone(), o.line0 + 1)).or_default().push(o.symbol.as_str());
        }
    }
    let syn = call_rows(&syn_db);
    let sym_t = syn_db.get_table("sym").unwrap();
    let local_crates: HashSet<String> = sym_t.all().find().unwrap_or_default().into_iter().map(|e| txt(sym_t.entity(e).get("crate_")).replace('-', "_")).collect();
    // res ごとに (確定数, 一致, 不一致, RA 未確定)
    let mut by_res: std::collections::BTreeMap<u32, [usize; 4]> = std::collections::BTreeMap::new();
    let mut wrong: Vec<String> = Vec::new();
    for (key, rows) in &syn {
        let [(Some(got), res)] = rows.as_slice() else { continue };
        let rel = rel_of(&key.0, root);
        if !scip.has_doc(&rel) {
            continue; // bake 後に変わった / RA が解析していない file は正解が無い
        }
        let e = by_res.entry(*res).or_default();
        e[0] += 1;
        // その行で RA が同じ名前を指した symbol がちょうど 1 つなら、それが正解。
        let mut syms: Vec<&str> = on_line.get(&(rel, key.1)).map(|v| v.iter().copied().filter(|s| scip_name(s) == key.2).collect()).unwrap_or_default();
        syms.sort_unstable();
        syms.dedup();
        let ra_says = match syms.as_slice() {
            [s] => match def_at.get(s) {
                Some(Some((r, l))) => Some(format!("{}/{r}:{l}", root.trim_end_matches('/'))),
                Some(None) => None, // target 間の衝突 = 判定不能
                // repo の crate の symbol なのに定義が読めない = 定義側の file が bake 後に変わった → 判定不能
                None if local_crates.contains(&scip_package(s).replace('-', "_")) => None,
                None => Some("outside the repo (std / deps)".to_string()),
            },
            _ => None, // RA が沈黙 / 同じ行に同名が複数
        };
        match ra_says {
            Some(w) if w == format!("{}:{}", got.0, got.1) => e[1] += 1,
            Some(w) => {
                e[2] += 1;
                if wrong.iter().filter(|w| w.ends_with(&format!("({})", RES_NAMES.get(*res as usize).unwrap_or(&"?")))).count() < std::env::var("KENNING_INFER_SHOW").ok().and_then(|v| v.parse().ok()).unwrap_or(8) {
                    wrong.push(format!("{}:{}\t{} → syn {}:{} / RA {w} ({})", key.0, key.1, key.2, got.0, got.1, RES_NAMES.get(*res as usize).unwrap_or(&"?")));
                }
            }
            None => e[3] += 1,
        }
    }
    let n_method = syn.values().flatten().filter(|(_, r)| *r == R_METHOD).count();
    let (rs_syn, rs_ra) = (ResolveStats::of(&syn_db.get_table("call").unwrap()), ResolveStats::of(&baked_db.get_table("call").unwrap()));
    println!("## ⑥ type inference check (syn-layer confirmed vs rust-analyzer)\n");
    println!(
        "in-repo confirmed rate: **syn layer alone {:.1}%** (confirmed {} / in repo {}) vs bake {:.1}% — reached without RA: {:.0}%\n",
        rs_syn.local_pct(), rs_syn.confirmed(), rs_syn.local(), rs_ra.local_pct(),
        if rs_ra.confirmed() > 0 { rs_syn.confirmed() as f64 * 100.0 / rs_ra.confirmed() as f64 } else { 0.0 }
    );
    println!("| res | confirmed by syn | agrees with RA | **disagrees (wrong)** | RA unresolved |");
    println!("|---|---|---|---|---|");
    for (r, [n, ok, bad, unk]) in &by_res {
        println!("| {} | {n} | {ok} | **{bad}** | {unk} |", RES_NAMES.get(*r as usize).unwrap_or(&"?"));
    }
    println!("\nleft as candidates, receiver type unknown (method-name): {n_method}\n");
    for w in &wrong {
        println!("- {w}");
    }
    drop(syn_db);
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_file(format!("{}.index.lock", tmp.display()));
}
