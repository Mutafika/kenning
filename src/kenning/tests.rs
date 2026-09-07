//! kenning の unit test。対象 module の private も root 再 export 経由で見える。

#![cfg(test)]

use super::*;

    /// enchudb 0.23+ の「満杯は拒否 + 計数」を黙って通さない: vocab を極小にして拒否を起こし、
    /// assert_no_faults が panic (= 呼び側の capacity リトライ / heal に落ちる) することを固定。
    #[test]
    fn assert_no_faults_panics_when_enchudb_rejected_writes() {
        let d = tmp_tree("faults");
        let path = d.join("f.db").to_string_lossy().to_string();
        let opts = enchudb::GrowableOptions { max_entities: 4096, vocab_max_entries: Some(8), ..Default::default() };
        let mut db = Database::create_growable_with(&path, opts).unwrap();
        db.table("t").tag("v").build().unwrap();
        let t = db.get_table("t").unwrap();
        for i in 0..64 {
            t.insert().set("v", format!("value-{i}").as_str()).commit().unwrap(); // Err にならず黙って拒否される
        }
        assert!(db.engine().fault_total() > 0, "vocab_max_entries=8 に 64 種を入れて fault が出ない");
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| assert_no_faults(&db, "index")));
        std::panic::set_hook(prev);
        let msg = r.unwrap_err().downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.contains("vocabulary full"), "拒否の内訳が出ない: {msg}");
        drop(t);
        drop(db);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 提案は「近い」の根拠が要る: 短すぎる定義名 (`_` / `Op`) は何にでも含まれるので 0 点。
    #[test]
    fn suggest_score_ignores_tiny_definition_names() {
        let lname = "copy_sparse";
        let tokens = suggest_tokens(lname);
        assert_eq!(tokens, vec!["copy", "sparse"]);
        assert_eq!(suggest_score(&tokens, lname, "_"), 0, "`_` が候補に出ていた");
        assert_eq!(suggest_score(&tokens, lname, "op"), 0, "`Op` が候補に出ていた");
        assert_eq!(suggest_score(&tokens, lname, "copy_db"), 1);
        assert_eq!(suggest_score(&tokens, lname, "decode_sparse_stream"), 1);
        assert_eq!(suggest_score(&tokens, lname, "copy_sparse_file"), 3, "両 token + substring 全体一致");
        assert_eq!(suggest_score(&tokens, lname, "sparse"), 2, "十分長い定義名の逆包含は生きる");
    }

    /// heal 用の最小 repo (Cargo.toml + src/a.rs) を作って index。(root, db) を返す。
    fn indexed_fixture(name: &str) -> (PathBuf, String) {
        let d = tmp_tree(name);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("Cargo.toml"), "[package]\nname = \"fx\"\nversion = \"0.0.0\"\n").unwrap();
        std::fs::write(d.join("src/a.rs"), "pub fn alpha() {}\n").unwrap();
        let db = d.join("k.db").to_string_lossy().to_string();
        run_index(&d.to_string_lossy(), &db, None);
        (std::fs::canonicalize(&d).unwrap(), db)
    }

    /// meta を消す/差し替える (旧版 index や repo 移動の再現)。
    fn rewrite_meta(db: &str, root: Option<&str>) {
        let d = Database::open(db).unwrap();
        let t = d.get_table("meta").unwrap();
        for e in t.all().find().unwrap() {
            t.entity(e).delete().unwrap();
        }
        if let Some(r) = root {
            t.insert().set("root", r).set("built_at", now_secs()).set("nfiles", 1u32).set("ver", INDEX_VER).commit().unwrap();
        }
    }

    fn meta_root(db: &str) -> Option<String> {
        read_meta(&Database::open_readonly(db).unwrap()).map(|(r, _)| r)
    }

    /// 鮮度を確認できない index (meta 無し / root 消失) は、root が分かる (自動導出 db) なら
    /// 警告でなく full 再 index で自己修復する。明示 db (root 不明) は従来通り触らない。
    #[test]
    fn auto_update_heals_unverifiable_index_only_when_root_is_known() {
        let (root, db) = indexed_fixture("heal");
        let root_s = root.to_string_lossy().to_string();
        assert_eq!(meta_root(&db).as_deref(), Some(root_s.as_str()));

        // 旧版 (meta 無し): 明示 db は警告のみ → meta は無いまま
        rewrite_meta(&db, None);
        maybe_auto_update(&db, None);
        assert_eq!(meta_root(&db), None, "明示 db を勝手に焼き直してはいけない");
        // 自動導出 db (root 既知) は heal → meta 復活 + 定義も引ける
        maybe_auto_update(&db, Some(&root));
        assert_eq!(meta_root(&db).as_deref(), Some(root_s.as_str()), "meta 無しの旧版が heal されていない");
        let d = Database::open_readonly(&db).unwrap();
        assert_eq!(d.get_table("sym").unwrap().where_eq("name", "alpha").find().unwrap().len(), 1);
        drop(d);

        // root 消失 (repo を移動): 同じく root 既知なら焼き直して root が正しくなる
        rewrite_meta(&db, Some("/nonexistent/kenning-moved-away"));
        maybe_auto_update(&db, Some(&root));
        assert_eq!(meta_root(&db).as_deref(), Some(root_s.as_str()), "root 消失の index が heal されていない");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// enchudb v10 で db が **directory** になったので、旧 binary が残した v9 の 1 ファイル db は
    /// open 段階で弾かれる。root が分かる自動導出 db なら黙って焼き直して復帰すること、隣に
    /// 残った v9 sidecar を回収すること、棚卸しで「壊れている」でなく「旧 format」に見えることを固定。
    #[test]
    fn legacy_v9_single_file_db_is_reindexed_not_reported_as_broken() {
        let (root, db) = indexed_fixture("legacy-v9");
        let root_s = root.to_string_lossy().to_string();
        assert!(Path::new(&db).is_dir(), "v10 の db は directory のはず");

        // v9 相当を再現: db を単一ファイルに戻し、sidecar を隣に置く
        let stage = || {
            let _ = std::fs::remove_dir_all(&db);
            std::fs::write(&db, b"not-a-v10-directory").unwrap();
            for ext in V9_SIDECAR_EXTS {
                std::fs::write(format!("{db}.{ext}"), b"stale").unwrap();
            }
        };
        stage();

        // 明示 db (root 不明) は従来通り触らない
        maybe_auto_update(&db, None);
        assert!(Path::new(&db).is_file(), "明示 db を勝手に焼き直してはいけない");

        // 自動導出 db (root 既知) は焼き直して復帰 — 定義も引ける
        maybe_auto_update(&db, Some(&root));
        assert!(Path::new(&db).is_dir(), "v9 の db が v10 に焼き直されていない");
        assert_eq!(meta_root(&db).as_deref(), Some(root_s.as_str()));
        let d = Database::open_readonly(&db).unwrap();
        assert_eq!(d.get_table("sym").unwrap().where_eq("name", "alpha").find().unwrap().len(), 1);
        drop(d);
        for ext in V9_SIDECAR_EXTS {
            assert!(!Path::new(&format!("{db}.{ext}")).exists(), "v9 の隣置き sidecar .{ext} が残っている");
        }

        // 棚卸し: 開けないからと放置せず、回収対象として数える
        stage();
        let scip = scip_path_of(&db);
        std::fs::write(&scip, b"baked").unwrap();
        let cache = Path::new(&db).parent().unwrap().to_path_buf();
        let e = cache_entries(&cache).into_iter().find(|e| e.db.to_string_lossy() == db).unwrap();
        assert!(e.status == CacheStatus::LegacyFile, "v9 が旧 format と判定されていない");
        assert!(cache_prunable(&e, None).is_some(), "旧 format が prune 対象になっていない");
        // 回収しても bake 成果物は残す (repo は生きていて、次の heal で再利用する)
        cache_prune(&cache, None, false);
        assert!(!Path::new(&db).exists(), "旧 format の db が回収されていない");
        assert!(Path::new(&scip).exists(), "bake 済み .scip まで道連れにしている");
        for ext in V9_SIDECAR_EXTS {
            assert!(!Path::new(&format!("{db}.{ext}")).exists(), "v9 の隣置き sidecar .{ext} が残っている");
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// v10 の db は directory なので、`update <dir> <db>` の振り分けを is_dir だけで決めると
    /// db を repo と誤認する (指定 db が更新されず cache に別 index が生える)。構造で見分けること。
    #[test]
    fn v10_db_directory_is_not_mistaken_for_a_repo() {
        let (root, db) = indexed_fixture("dbdir");
        assert!(Path::new(&db).is_dir(), "v10 の db は directory のはず");
        assert!(is_db_path(&db), "db を db と判別できていない");
        assert!(!is_db_path(&root.to_string_lossy()), "repo root を db と誤認している");
        // repo root は probe 上 Incomplete (header.seg が無い)。db 側に倒すと全 repo が db になる。
        assert!(Engine::probe(&root) == DbState::Incomplete, "前提が変わった: repo root の probe 結果");
        // v9 の単一ファイルも db 側 (repo と誤認して索引しにいかない)
        let v9 = root.join("legacy-v9-shaped.db");
        std::fs::write(&v9, b"x").unwrap();
        assert!(is_db_path(&v9.to_string_lossy()), "v9 の単一ファイル db を db と判別できていない");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 同じ db への full index は process 間で直列化され、待った側は勝った側の成果を再利用する。
    /// 直列化が無いと並列 heal が互いの作りかけ directory を消し合う (v10 で顕在化、3 本中 1 本しか
    /// 答えず 1 本は panic、を再現済み)。「他 process が焼いている最中」は IndexLock を手で握って
    /// 再現する (flock は open file description 単位なので、同一 process 内の別 open でも衝突する)。
    #[test]
    fn ensure_index_waits_for_concurrent_indexer_and_reuses_its_result() {
        let (root, db) = indexed_fixture("waitreuse");
        let root_s = root.to_string_lossy().to_string();
        let spawn = |explicit: bool| {
            let (r, d) = (root_s.clone(), db.clone());
            std::thread::spawn(move || index_locked(&r, &d, None, !explicit))
        };
        let settle = || std::thread::sleep(std::time::Duration::from_millis(150));

        // 勝った側が現行版を残した → 待った側は焼き直さない
        let held = IndexLock::acquire(&db).unwrap();
        assert!(!held.1, "誰も握っていないのに待ち扱い");
        let t = spawn(false);
        settle();
        assert!(Path::new(&db).is_dir(), "lock 待ち中に db が触られている");
        drop(held);
        assert!(!t.join().unwrap(), "待った側が現行版を再利用せず焼き直している");

        // 勝った側が失敗して db が無い → 待った側が自分で焼く
        let held = IndexLock::acquire(&db).unwrap();
        let t = spawn(false);
        settle();
        wipe_db(&db);
        drop(held);
        assert!(t.join().unwrap(), "db が無いのに焼いていない");
        assert_eq!(meta_root(&db).as_deref(), Some(root_s.as_str()));

        // 明示 run_index (bake / `kenning index`) は待っても必ず焼く
        let held = IndexLock::acquire(&db).unwrap();
        let t = spawn(true);
        settle();
        drop(held);
        assert!(t.join().unwrap(), "明示 index が再利用で済まされている");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 作りかけは本番 path に置かない: `<db>.tmp-<pid>` に焼いて rename で差し替える。
    /// crash した process の残骸 (`.tmp-` / `.old-`) は次の index が lock 下で回収する。
    /// v9 の単一ファイル db に 3 本同時に heal が走っても、全員が生還し db は 1 つ・残骸ゼロ。
    #[test]
    fn index_builds_aside_sweeps_leftovers_and_survives_parallel_heal() {
        let (root, db) = indexed_fixture("aside");
        let root_s = root.to_string_lossy().to_string();
        // crash 残骸を仕込む
        let (tmp_leftover, old_leftover) = (format!("{db}.tmp-99999"), format!("{db}.old-99999"));
        std::fs::create_dir_all(&tmp_leftover).unwrap();
        std::fs::write(&old_leftover, b"x").unwrap();
        // v9 相当 (単一ファイル) に戻して 3 本並列で heal
        std::fs::remove_dir_all(&db).unwrap();
        std::fs::write(&db, b"not-a-v10-directory").unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let hs: Vec<_> = (0..3)
            .map(|_| {
                let (r, d, b) = (root.clone(), db.clone(), barrier.clone());
                std::thread::spawn(move || {
                    b.wait();
                    maybe_auto_update(&d, Some(&r));
                })
            })
            .collect();
        for h in hs {
            h.join().expect("並列 heal で panic");
        }
        assert!(Path::new(&db).is_dir(), "v9 → v10 に焼き直されていない");
        assert_eq!(meta_root(&db).as_deref(), Some(root_s.as_str()));
        assert!(!Path::new(&tmp_leftover).exists() && !Path::new(&old_leftover).exists(), "crash 残骸が回収されていない");
        let leftovers: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp-") || n.contains(".old-"))
            .collect();
        assert!(leftovers.is_empty(), "作りかけ / 退避が残っている: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 増分 update は mtime を信じられる時、index 時刻より古い既知ファイルを読まない (tokio 725 files で
    /// 全件読み + hash 25〜30ms → 数 ms)。代償として mtime の巻き戻し (rsync -a / touch -t) は
    /// mtime 信頼中は見えない — それは trust_mtime=false (明示 update / 7 日超の db) の全件 hash 照合で
    /// 拾う。両方の振る舞いと「普通の編集は mtime 信頼中でも拾う」を固定。
    #[test]
    fn incremental_update_trusts_mtime_but_full_check_catches_rollback() {
        let (root, db) = indexed_fixture("mtime");
        let root_s = root.to_string_lossy().to_string();
        let a = root.join("src/a.rs");
        let count = |name: &str| {
            let d = Database::open_readonly(&db).unwrap();
            d.get_table("sym").unwrap().where_eq("name", name).find().unwrap().len()
        };
        let built_at = read_meta(&Database::open_readonly(&db).unwrap()).map(|(_, b)| b).unwrap();

        // 内容は変えたが mtime を index より前に巻き戻す (rsync -a / touch -t 相当)
        std::fs::write(&a, "pub fn alpha() {}\npub fn beta() {}\n").unwrap();
        let rolled = std::time::UNIX_EPOCH + std::time::Duration::from_secs(built_at as u64 - 100);
        std::fs::File::options().write(true).open(&a).unwrap().set_modified(rolled).unwrap();
        update_with_heal(Database::open(&db).unwrap(), &root_s, &db, UpdateScan::Walk { trust_mtime: true }, "test");
        assert_eq!(count("beta"), 0, "mtime 信頼中に古い mtime のファイルを読んでいる (絞り込みが効いていない)");
        update_with_heal(Database::open(&db).unwrap(), &root_s, &db, UpdateScan::Walk { trust_mtime: false }, "test");
        assert_eq!(count("beta"), 1, "全件 hash 照合が mtime 巻き戻しを拾えていない");

        // 普通の編集 (mtime = 今) は mtime 信頼中でも拾う。新規ファイルも同様
        std::fs::write(&a, "pub fn alpha() {}\npub fn beta() {}\npub fn gamma() {}\n").unwrap();
        std::fs::write(root.join("src/b.rs"), "pub fn delta() {}\n").unwrap();
        update_with_heal(Database::open(&db).unwrap(), &root_s, &db, UpdateScan::Walk { trust_mtime: true }, "test");
        assert_eq!((count("gamma"), count("delta")), (1, 1), "mtime 信頼中に普通の編集 / 新規ファイルを取りこぼした");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// dir ゲート: 既知 dir と file の stat だけで「walk 不要」を判定する。編集 → Stale (その file だけ)、
    /// 追加 / 削除 / .gitignore 編集 → DirsChanged (walk に落ちる)、何もなければ Fresh。
    /// Stale 経路の update は walk せずその file だけ読んで反映する。
    #[test]
    fn dir_gate_skips_walk_unless_entries_changed() {
        let (root, db) = indexed_fixture("gate");
        let root_s = root.to_string_lossy().to_string();
        let known = || load_known(&Database::open_readonly(&db).unwrap()).expect("dir 表が無い");
        let k = known();
        assert!(k.dirs.iter().any(|d| d == &root) && k.dirs.iter().any(|d| d.ends_with("src")), "dir 表に root / src が無い: {:?}", k.dirs);
        assert_eq!(k.files.len(), 2, "Cargo.toml + src/a.rs のはず: {:?}", k.files);
        // fixture は index と同じ秒に書かれている (= `>=` で Stale 扱い) ので、全部 100 秒前に巻き戻して
        // 「何もしていない」状態を作る。
        let now = now_secs();
        let backdate = |p: &Path| {
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(now as u64 - 100);
            std::fs::File::open(p).unwrap().set_modified(t).unwrap();
        };
        let settle = |k: &Known| k.dirs.iter().chain(k.files.iter()).for_each(|p| backdate(p));
        settle(&k);
        assert!(matches!(fs_gate(&k, now), Gate::Fresh), "何もしていないのに Fresh でない");

        // 既知 file の編集 → その file だけ Stale
        let a = root.join("src/a.rs");
        std::fs::write(&a, "pub fn alpha() {}\npub fn beta() {}\n").unwrap();
        match fs_gate(&k, now) {
            Gate::Stale(files) => assert_eq!(files, vec![a.clone()]),
            _ => panic!("編集した file が Stale に出ていない"),
        }
        // Stale 経路の update: walk せずその file だけ読んで反映
        update_with_heal(Database::open(&db).unwrap(), &root_s, &db, UpdateScan::Stale(vec![a.clone()]), "test");
        let d = Database::open_readonly(&db).unwrap();
        assert_eq!(d.get_table("sym").unwrap().where_eq("name", "beta").find().unwrap().len(), 1, "Stale 経路で編集が反映されていない");
        drop(d);

        // 追加 → dir の mtime が動く → DirsChanged
        settle(&known());
        std::fs::write(root.join("src/b.rs"), "pub fn delta() {}\n").unwrap();
        assert!(matches!(fs_gate(&known(), now), Gate::DirsChanged), "file 追加が DirsChanged にならない");
        // 削除も同様
        settle(&known());
        std::fs::remove_file(root.join("src/b.rs")).unwrap();
        assert!(matches!(fs_gate(&known(), now), Gate::DirsChanged), "file 削除が DirsChanged にならない");
        // .gitignore の in-place 編集は dir の mtime を動かさないが、個別に見て DirsChanged
        let gi = root.join(".gitignore");
        std::fs::write(&gi, "").unwrap();
        settle(&known());
        backdate(&gi);
        assert!(matches!(fs_gate(&known(), now), Gate::Fresh), "巻き戻した .gitignore が引っかかっている");
        std::fs::write(&gi, "target/\n").unwrap();
        assert!(matches!(fs_gate(&known(), now), Gate::DirsChanged), ".gitignore 編集が DirsChanged にならない");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// cache の棚卸し: root が消えた index だけが prune 対象。sidecar (v10 は db directory の
    /// 中、bake 済み `.scip` は隣) ごと消える。
    #[test]
    fn cache_prune_removes_only_index_whose_root_is_gone() {
        let cache = tmp_tree("cache");
        let (root_a, _) = indexed_fixture("cache-a");
        let (root_b, _) = indexed_fixture("cache-b");
        let db_a = cache.join("a-00000001.db").to_string_lossy().to_string();
        let db_b = cache.join("b-00000002.db").to_string_lossy().to_string();
        run_index(&root_a.to_string_lossy(), &db_a, None);
        run_index(&root_b.to_string_lossy(), &db_b, None);
        std::fs::write(cache.join("b-00000002.scip"), b"stub").unwrap(); // bake 済み sidecar も道連れ
        std::fs::write(cache.join("bake.lock"), b"1").unwrap(); // 無関係ファイルは触らない
        std::fs::remove_dir_all(&root_b).unwrap();

        let es = cache_entries(&cache);
        assert_eq!(es.len(), 2);
        assert!(es[0].status == CacheStatus::Ok && es[0].db.to_string_lossy() == db_a);
        assert!(es[1].status == CacheStatus::RootMissing && es[1].db.to_string_lossy() == db_b);
        assert!(es[1].bytes > 0, "実消費を数えている");

        assert_eq!(cache_prune(&cache, None, true), 1, "dry-run でも対象は数える");
        assert!(Path::new(&db_b).exists(), "dry-run は消さない");
        assert_eq!(cache_prune(&cache, None, false), 1);
        assert!(!Path::new(&db_b).exists());
        assert!(!cache.join("b-00000002.scip").exists(), "bake 済み sidecar が道連れになっていない");
        assert!(Path::new(&db_a).exists() && cache.join("bake.lock").exists());
        // 生きている index も --older-than 0 (= 今日以前すべて) なら対象
        assert_eq!(cache_prune(&cache, Some(0), true), 1);
        for d in [&cache, &root_a] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn text_containers_md_tracks_heading_hierarchy() {
        let src = "# 設計原則\nintro\n## 紐が本質\nbody\n### 詳細\nx\n## 別の節\ny\n#notaheading\n";
        let got = text_containers(LANG_MD, src);
        assert_eq!(got[0], (1, "設計原則".to_string()));
        assert_eq!(got[1], (3, "設計原則 > 紐が本質".to_string()));
        assert_eq!(got[2], (5, "設計原則 > 紐が本質 > 詳細".to_string()));
        // 浅い見出しに戻ると深い方は落ちる
        assert_eq!(got[3], (7, "設計原則 > 別の節".to_string()));
        // `#` 直後に空白が無いものは見出しではない (Rust の属性や C の #include を拾わない)
        assert_eq!(got.len(), 4);
    }

    #[test]
    fn text_containers_toml_and_yaml() {
        let toml = "x = 1\n[package]\nname = \"a\"\n[[bin]]\nname = \"b\"\n";
        assert_eq!(text_containers(LANG_TOML, toml), vec![(2, "package".to_string()), (4, "bin".to_string())]);

        let yaml = "jobs:\n  build:\n    runs-on: ubuntu\n  test:\n    x: 1\n";
        let got = text_containers(LANG_YAML, yaml);
        assert_eq!(got[1], (2, "jobs.build".to_string()));
        assert_eq!(got[2], (3, "jobs.build.runs-on".to_string()));
        // インデントが戻れば深い方は落ちる
        assert_eq!(got[3], (4, "jobs.test".to_string()));

        // 抽出関数を持たない形式は注釈なし
        assert!(text_containers(LANG_TEXT, "anything\ngoes\n").is_empty());
    }

    #[test]
    fn text_files_skips_rs_binary_and_oversize() {
        let d = tmp_tree("textfiles");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.rs"), "fn main() {}").unwrap();
        std::fs::write(d.join("README.md"), "# hi").unwrap();
        std::fs::write(d.join("sub/conf.toml"), "[x]").unwrap();
        std::fs::write(d.join("noext"), "#!/bin/sh").unwrap(); // 拡張子なしも対象 (形式は問わない)
        std::fs::write(d.join("Cargo.lock"), "[[package]]").unwrap(); // 生成物は索引しない
        std::fs::write(d.join("blob.dat"), [1u8, 0, 2, 3]).unwrap(); // NUL 入り = binary
        std::fs::write(d.join("big.txt"), vec![b'x'; (TEXT_MAX_BYTES + 1) as usize]).unwrap();

        let mut got: Vec<String> = text_files(&d.to_string_lossy()).map(|p| rel_in(&d, &p)).collect();
        got.sort();
        // .rs は rust_files 側、big.txt はサイズ上限で walk 段階から落ちる
        assert_eq!(got, ["README.md", "blob.dat", "noext", "sub/conf.toml"]);
        // binary は読み込み段で落ちる (walk では拡張子で判断しない)
        assert!(read_text_file(&d.join("blob.dat")).is_none());
        assert!(read_text_file(&d.join("README.md")).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn peak_mb_from_time_stats_file() {
        let stats = "        0.53 real         0.31 user         0.11 sys\n          2598292024  maximum resident set size\n                   0  average shared memory size\n";
        assert_eq!(parse_peak_mb(stats), Some(2477));
        assert_eq!(parse_peak_mb("no stats here\n"), None);
    }

    #[test]
    fn ra_error_lines_picks_panic_not_time_footer() {
        // 実例 (#3): panic は stderr の先頭側、time の統計は末尾 → 末尾 5 行では真因が消える。
        let errs = "\
Loading metadata...
thread 'main' panicked at crates/rust-analyzer/src/cli/scip.rs:227:17:
Invariant violation: file emitted multiple times.
note: run with `RUST_BACKTRACE=1` to display a backtrace
               41390  voluntary context switches
              590206  involuntary context switches
        209737249375  instructions retired
         71152919364  cycles elapsed
          2598292024  peak memory footprint";
        let got = ra_error_lines(errs);
        assert!(got.contains("panicked at"), "panic 行を拾えていない: {got}");
        // panic 行の次行 (メッセージ本体) も連れてくる
        assert!(got.contains("Invariant violation: file emitted multiple times."), "{got}");
        assert!(!got.contains("cycles elapsed"), "time の統計を混ぜている: {got}");
    }

    #[test]
    fn ra_error_lines_falls_back_to_tail() {
        // マーカーが一つも無ければ従来通り末尾 5 行
        let errs = "a\nb\nc\nd\ne\nf\ng";
        assert_eq!(ra_error_lines(errs), "c\nd\ne\nf\ng");
        assert!(ra_error_lines("").contains("stderr に何も出さなかった"));
    }

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

    /// walk の結果を root からの相対で見る (assert を読みやすく)。
    fn rel_in(root: &Path, p: &Path) -> String {
        p.strip_prefix(root).unwrap().to_string_lossy().into_owned()
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
        let mut got: Vec<String> = rust_files(&d.to_string_lossy()).map(|p| rel_in(&d, &p)).collect();
        got.sort();
        assert_eq!(got, ["src/a.rs", "sub/b.rs"]);
        // root 自身が隠し名でも depth 0 は素通し (直指定で index できる)
        let hidden_root = d.join(".store/blobs");
        assert_eq!(rust_files(&hidden_root.to_string_lossy()).count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// #4: bake に渡すのは repo root ではなく「cwd を含む cargo workspace」。
    /// rust-analyzer は 1 project しか読めず、root が cargo project でない repo では黙って別 crate を
    /// 焼く / `more than one project` で落ちる。死んだ tree (.attic 等) は index_walk が最初から外す。
    #[test]
    fn bake_target_is_workspace_not_repo_root() {
        let d = tmp_tree("baketarget");
        let ws = d.join("ws");
        std::fs::create_dir_all(ws.join("crates/core/src")).unwrap();
        std::fs::create_dir_all(d.join(".attic/dead")).unwrap(); // 隠し dir = 死んだ tree
        std::fs::write(ws.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/*\"]\n").unwrap();
        std::fs::write(ws.join("crates/core/Cargo.toml"), "[package]\nname = \"core\"\n").unwrap();
        std::fs::write(d.join(".attic/dead/Cargo.toml"), "[package]\nname = \"dead\"\n").unwrap();

        // member から叩いても workspace root を焼く (member 単体だと cross-crate が解けない)
        let got = bake_target_of(&ws.join("crates/core").to_string_lossy(), &d).unwrap();
        assert_eq!(got, std::fs::canonicalize(&ws).unwrap());
        // repo root には Cargo.toml が無い → 生きている project は ws だけなのでそれを選ぶ
        let got = bake_target_of(&d.to_string_lossy(), &d).unwrap();
        assert_eq!(got, std::fs::canonicalize(&ws).unwrap());
        assert_eq!(cargo_projects_under(&d).len(), 1, ".attic の死んだ project を数えている");

        // 独立 project が 2 つ = RA では決められない → 黙って 1 つ選ばず、選択を促して落とす
        std::fs::create_dir_all(d.join("other/src")).unwrap();
        std::fs::write(d.join("other/Cargo.toml"), "[package]\nname = \"other\"\n").unwrap();
        let err = bake_target_of(&d.to_string_lossy(), &d).unwrap_err();
        assert!(err.contains("2 個"), "{err}");
        assert!(err.contains("cd"), "どこへ cd すべきか出ていない: {err}");
        // cwd が片方の中なら曖昧さは無い
        let got = bake_target_of(&d.join("other/src").to_string_lossy(), &d).unwrap();
        assert_eq!(got, std::fs::canonicalize(d.join("other")).unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// #13: index の**開始**時刻を built_at に焼き、同一秒の編集も stale に数える。
    /// 完了時刻を焼く + `>` 比較だと、index 実行中に編集されたファイルが「mtime < built_at」に
    /// 収まって以後の鮮度判定を永久にすり抜け、「シンボルは正しいのに行番号だけ古い」が固定する。
    #[test]
    fn walk_stats_counts_edit_made_during_index() {
        let d = tmp_tree("stale");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/a.rs"), "fn a() {}").unwrap();
        let started_at = now_secs(); // = index 開始時刻 (built_at に焼く値)
        std::fs::write(d.join("src/a.rs"), "fn a() {}\nfn b() {}").unwrap(); // index 中の編集
        let (stale, seen) = walk_stats(&d.to_string_lossy(), started_at);
        assert_eq!(seen, 1, "走査できたファイル数");
        assert_eq!(stale, 1, "index 開始と同じ秒の編集を取りこぼしている");
        // root が消えていれば seen=0 → 呼び側はこれを「最新」ではなく「確認できない」と扱う
        assert_eq!(walk_stats(&d.join("nope").to_string_lossy(), started_at), (0, 0));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// #12: gitignore 済みパスは .rs / 非 .rs のどちらの walk からも落ちる (= rg と同じ規約)。
    /// 生成物の展開先や vendor 複製が index に入ると、本体とほぼ同一のコピーが def の重複シンボルに
    /// なる。`text` 側 (.md) だけ落ちて `.rs` は出る、という**片肺の不整合**も同時に見張る。
    #[test]
    fn walks_respect_gitignore_for_rs_and_text() {
        let d = tmp_tree("gitignore");
        for sub in [".git", "src", "vendor/src"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        std::fs::write(d.join(".gitignore"), "/vendor/\n").unwrap();
        for f in ["src/lib.rs", "vendor/src/lib.rs"] {
            std::fs::write(d.join(f), "pub fn helper_fn() {}").unwrap();
        }
        for f in ["notes.md", "vendor/notes.md"] {
            std::fs::write(d.join(f), "helper_fn").unwrap();
        }
        let rs: Vec<String> = rust_files(&d.to_string_lossy()).map(|p| rel_in(&d, &p)).collect();
        assert_eq!(rs, ["src/lib.rs"], "gitignore 済みの .rs が index 対象に残っている");
        let mut txt: Vec<String> = text_files(&d.to_string_lossy()).map(|p| rel_in(&d, &p)).collect();
        txt.sort();
        assert_eq!(txt, ["notes.md"], "非 .rs 側の規約が .rs 側とずれている"); // .gitignore 自身は隠しファイル

        // 逃げ道: gitignore 済みだが実際に compile される生成 .rs を持つ repo 向け。
        // (env は同一プロセス内の他テストに漏れるため、この 1 テスト内で set → 即 remove)
        unsafe { std::env::set_var("KENNING_NO_IGNORE", "1") };
        let n = rust_files(&d.to_string_lossy()).count();
        unsafe { std::env::remove_var("KENNING_NO_IGNORE") };
        assert_eq!(n, 2, "KENNING_NO_IGNORE=1 で ignore 規則を外せていない");
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
    fn text_hit_count_reads_the_count_line_in_every_shape() {
        // 通常 / path: 絞り / limit 省略あり / 0 件 — bench の一致判定はこの数字に乗る
        assert_eq!(super::text_hit_count("a:1\tx\n# 4 件 / 3 files\n"), 4);
        assert_eq!(super::text_hit_count("# 3 件 / 2 files (path: docs/ の 2 files)\n"), 3);
        assert_eq!(super::text_hit_count("# 68 件 / 12 files — 表示 50、`--limit 68` で全部\n"), 68);
        assert_eq!(super::text_hit_count("# \"zz\" は index 済みファイルに無い (.rs + テキスト全般)\n"), 0);
        assert_eq!(super::text_hit_count(""), 0);
    }

    #[test]
    fn bench_medians_and_lcg_deterministic() {
        assert_eq!(median_u(vec![5, 1, 3]), 3);
        assert_eq!(median_u(vec![]), 0);
        assert!((median_f(vec![2.0, 1.0, 4.0]) - 2.0).abs() < 1e-9);
        let (mut a, mut b) = (Lcg(42), Lcg(42));
        assert_eq!((a.next(), a.next()), (b.next(), b.next())); // 同 seed → 同列 (再現性)
    }

    /// bake の上限: 子 process を group ごと落として None、普通に終われば Some(成功)。
    #[cfg(unix)]
    #[test]
    fn run_with_timeout_kills_process_group_and_reports_normal_exit() {
        let d = std::env::temp_dir().join(format!("kenning-rwt-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let err = d.join("err").to_string_lossy().to_string();
        let t = Instant::now();
        // sh が sleep を子に持つ = group kill が要る形。timeout 後に孫まで消える。
        let mut c = std::process::Command::new("sh");
        c.args(["-c", "sleep 30 & wait"]);
        let r = run_with_timeout(c, &err, Duration::from_millis(300)).unwrap();
        assert!(r.is_none(), "timeout なのに完了扱い: {r:?}");
        assert!(t.elapsed() < Duration::from_secs(5), "kill が効いていない ({:?})", t.elapsed());
        let mut c = std::process::Command::new("sh");
        c.args(["-c", "echo boom >&2; exit 3"]);
        let r = run_with_timeout(c, &err, Duration::from_secs(10)).unwrap().expect("完了するはず");
        assert_eq!(r.code(), Some(3));
        assert_eq!(std::fs::read_to_string(&err).unwrap().trim(), "boom", "stderr が file に落ちていない");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `target/` と `node_modules/` は gitignore の有無に関係なく降りない (git 管理外の dir でも)。
    #[test]
    fn walk_always_prunes_target_and_node_modules() {
        let d = std::env::temp_dir().join(format!("kenning-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for sub in ["src", "target/debug", "node_modules/pkg", "mcp/node_modules/x"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        std::fs::write(d.join("src/a.rs"), "pub fn a() {}\n").unwrap();
        std::fs::write(d.join("target/debug/b.rs"), "pub fn b() {}\n").unwrap();
        std::fs::write(d.join("node_modules/pkg/c.rs"), "pub fn c() {}\n").unwrap();
        std::fs::write(d.join("node_modules/pkg/README.md"), "# c\n").unwrap();
        std::fs::write(d.join("mcp/node_modules/x/index.js"), "x\n").unwrap();
        std::fs::write(d.join("README.md"), "# root\n").unwrap();
        let ws = walk_index_set(&d.to_string_lossy());
        let names = |v: &[PathBuf]| v.iter().map(|p| p.strip_prefix(&d).unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
        assert_eq!(names(&ws.rs), ["src/a.rs"], "rs: {:?}", names(&ws.rs));
        assert_eq!(names(&ws.text), ["README.md"], "text: {:?}", names(&ws.text));
        assert!(names(&ws.dirs).iter().all(|p| !p.contains("node_modules") && !p.contains("target")), "dirs: {:?}", names(&ws.dirs));
        let _ = std::fs::remove_dir_all(&d);
    }

/// file 冒頭の `#![cfg(test)]` は file 全体に効く。per-file parse では親の `#[cfg(test)] mod x;` が
/// 見えないので、これを拾わないと test 専用 file の helper が「test でない symbol」になり、
/// `tests <name>` / `search test:1` が取りこぼす (この repo の tests.rs 自体がその形)。
#[test]
fn file_level_cfg_test_marks_every_symbol_in_the_file() {
    let f = extract_file("#![cfg(test)]\nfn helper() {}\n#[test]\nfn t() { helper(); }\n").unwrap();
    assert_eq!(f.syms.len(), 2, "{:?}", f.syms.iter().map(|s| &s.name).collect::<Vec<_>>());
    assert!(f.syms.iter().all(|s| s.is_test), "#![cfg(test)] の file は全部 test 扱い");
    // 付いていない file は従来通り #[test] だけが test
    let g = extract_file("fn helper() {}\n#[test]\nfn t() {}\n").unwrap();
    assert_eq!(g.syms.iter().filter(|s| s.is_test).count(), 1);
}

/// 提案の正規化層: 書き方違い (snake↔camel、大小) は同じキーに落ちる。
/// Claude の typo で最も多いのがこれで、token 層だけでは 1 件も救えていなかった。
#[test]
fn norm_key_folds_case_and_separators() {
    assert_eq!(norm_key("cmd_read"), "cmdread");
    assert_eq!(norm_key("CmdRead"), "cmdread");
    assert_eq!(norm_key("cmdRead"), "cmdread");
    assert_eq!(norm_key("TextMatcher"), "textmatcher");
    assert_ne!(norm_key("cmd_read"), norm_key("cmd_reads"));
}

/// 打ち切り付き編集距離: 1〜2 文字の typo だけを救い、遠い名前は拾わない (提案のノイズ防止)。
#[test]
fn edit_within_accepts_only_small_typos() {
    assert!(edit_within("cmdread", "cmdread", 1));
    assert!(edit_within("cmdreed", "cmdread", 1), "1 文字置換");
    assert!(edit_within("runindexiner", "runindexinner", 1), "1 文字欠落");
    assert!(!edit_within("cmdreed", "cmdread", 0));
    assert!(!edit_within("cmdread", "cmdwrite", 2), "遠い名前を拾うと提案が濁る");
    assert!(!edit_within("ab", "abcdef", 2), "長さ差での早期棄却");
}

/// 正規化層の配点: 完全一致 > typo > 部分名。短い名前ほど typo 許容を狭める (ノイズ抑制)。
#[test]
fn suggest_score_norm_ranks_spelling_over_substring() {
    assert_eq!(suggest_score_norm("cmdread", "cmdread"), 6, "書き方違いだけ = 最優先");
    assert_eq!(suggest_score_norm("cmdreed", "cmdread"), 5, "1 文字 typo");
    assert_eq!(suggest_score_norm("readtext", "readtextfile"), 2, "部分名");
    assert_eq!(suggest_score_norm("cmdread", "totallyunrelated"), 0);
    // 綴り一致は部分一致 (最大 2 + token 層) より必ず上 = 正解が候補の山に埋もれない
    assert!(suggest_score_norm("cmdreed", "cmdread") > suggest_score_norm("readtext", "readtextfile") + 2);
    assert_eq!(suggest_score_norm("abcd", "abce"), 5, "短い名前でも 1 文字違いは救う");
    assert_eq!(suggest_score_norm("cmdrefs", "cmdread"), 0, "短い名前の 2 文字違いは別 symbol とみなす");
}

/// `text` の一致判定: 既定は OR (1 本に畳む)、`--and` は全語が同じ行に要る。
#[test]
fn text_matcher_or_and_modes() {
    let terms = vec!["lock".to_string(), "db".to_string()];
    let or = TextMatcher::new(&terms, false, false).unwrap();
    assert_eq!(or.0.len(), 1, "OR は 1 本の regex に畳む (速い)");
    assert!(or.hit("just a lock"));
    assert!(or.hit("just a DB"), "大小無視");
    assert!(!or.hit("neither"));

    let and = TextMatcher::new(&terms, false, true).unwrap();
    assert_eq!(and.0.len(), 2);
    assert!(and.hit("lock the db"));
    assert!(!and.hit("just a lock"), "片方だけの行は AND では落ちる");

    // -e: 語を正規表現として扱う。AND と併用できる。
    let re = TextMatcher::new(&["fn cmd_\\w+".to_string(), "pub".to_string()], true, true).unwrap();
    assert!(re.hit("pub fn cmd_read(args: &[String]) {"));
    assert!(!re.hit("fn cmd_read(args: &[String]) {"));
}

/// `<path>:<from>-<to>` の解釈: `:` の後ろだけを見るので file 名の `-` と衝突しない。
/// 逆順は黙って直す (打ち間違いで「0 行」を返すより有用)。
#[test]
fn split_path_range_parses_line_ranges() {
    assert_eq!(split_path_range("src/a.rs:10-20"), Some(("src/a.rs", 10, 20)));
    assert_eq!(split_path_range("bench/vs-ra.sh:1-5"), Some(("bench/vs-ra.sh", 1, 5)), "file 名の - と衝突しない");
    assert_eq!(split_path_range("src/a.rs:20-10"), Some(("src/a.rs", 10, 20)), "逆順は直す");
    assert_eq!(split_path_range("src/a.rs:10"), None, "単一行は従来の path:line 経路へ");
    assert_eq!(split_path_range("src/a.rs:0-5"), None, "1-indexed なので 0 は無効");
    assert_eq!(split_path_range("Engine::run"), None, "symbol 名を範囲と誤読しない");
}

/// 修飾名の分解: kenning 自身が出す `Type::method` を、そのまま次のコマンドに渡せるようにする鍵。
#[test]
fn split_qualified_splits_type_and_method() {
    assert_eq!(split_qualified("Engine::open_readonly"), ("open_readonly", Some("Engine")));
    assert_eq!(split_qualified("a::b::c"), ("c", Some("b")), "直近の親を container にする");
    assert_eq!(split_qualified("cmd_read"), ("cmd_read", None));
    assert_eq!(split_qualified("::x"), ("::x", None), "壊れた形は素の名前として扱う");
    assert_eq!(split_qualified("x::"), ("x::", None));
}

/// bake が RA に渡す config は **立っているキーだけ**書く (空キーで RA の既定を潰さない)。
/// RUSTFLAGS はユーザー入力なので JSON として壊れない形に逃がす。
#[test]
fn ra_config_writes_only_the_keys_that_are_set() {
    assert_eq!(ra_config(true, ""), r#"{"cargo": {"features": "all"}}"#);
    assert_eq!(
        ra_config(false, "--cfg tokio_unstable"),
        r#"{"cargo": {"extraEnv": {"RUSTFLAGS": "--cfg tokio_unstable"}}}"#
    );
    assert_eq!(
        ra_config(true, "--cfg a"),
        r#"{"cargo": {"features": "all", "extraEnv": {"RUSTFLAGS": "--cfg a"}}}"#
    );
    assert!(ra_config(false, "--cfg x=\"y\"").contains(r#"x=\"y\""#), "引用符を escape する");
}
