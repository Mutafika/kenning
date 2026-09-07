//! syn AST → facts の抽出と、索引対象ファイルの walk。

use super::*;

// ─────────────────────────── indexer (syn) ───────────────────────────

pub(crate) fn classify_vis(v: &syn::Visibility) -> u32 {
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

pub(crate) fn is_test_attrs(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path()
            .segments
            .last()
            .is_some_and(|s| s.ident == "test") // #[test] / #[tokio::test] / #[…::test]
    })
}

pub(crate) fn is_cfg_test(attr: &syn::Attribute) -> bool {
    if !attr.path().is_ident("cfg") {
        return false;
    }
    // #[cfg(test)] / #[cfg(all(test, ...))] を雑に判定 (PoC 割り切り)
    attr.to_token_stream().to_string().contains("test")
}

pub(crate) fn line_of(span: proc_macro2::Span) -> u32 {
    span.start().line as u32 // proc-macro2: line は 1-indexed
}
pub(crate) fn col_of(span: proc_macro2::Span) -> u32 {
    span.start().column as u32 // proc-macro2: column は char 単位・0-indexed
}
/// item 全体 (body 含む) の終端行。`read` が定義本体を切り出す範囲の下端。
pub(crate) fn end_line_of<T: syn::spanned::Spanned>(t: &T) -> u32 {
    t.span().end().line as u32
}

/// SCIP 列(UTF-8 バイトオフセット) → proc-macro2 と同じ char 単位の列に変換。
/// rust-analyzer の SCIP は position_encoding=UTF8(バイト)だが syn は char 数を返すので、
/// 非 ASCII 行(enchudb は日本語コメント多数)では両者がズレて位置 join が黙って外れる。
/// ASCII 行は byte==char なので即返し(実測 occurrence の 97% はこの fast path)。
pub(crate) fn byte_col_to_char(line: &str, byte_col: u32) -> u32 {
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
pub(crate) fn rel_of(abs: &str, root: &str) -> String {
    let r = root.trim_end_matches('/');
    abs.strip_prefix(r).map(|s| s.trim_start_matches('/').to_string()).unwrap_or_else(|| abs.to_string())
}

/// impl の self_ty を基底型名に正規化 ("Foo < T >" → "Foo")。
pub(crate) fn clean_type(ty: &syn::Type) -> String {
    ty.to_token_stream()
        .to_string()
        .split('<')
        .next()
        .unwrap_or("")
        .replace(' ', "")
}

/// 1 呼び出し箇所。名前解決の材料として修飾 (`Type::` / `mod::`) も持つ。
pub(crate) struct RawCall {
    name: String,               // 呼び先の単純名 (path 末尾 / method 名)
    qualifier: Option<String>,  // path の末尾手前 seg (`Engine::new` の "Engine" / `Self`)。method 呼びは None
    is_method: bool,            // x.foo() 形式か
    self_recv: bool,            // `self.foo()` — 受け手が self = 現在の impl 型と確定できる唯一の形
    line: u32,                  // callee ident の行 (1-indexed)
    col: u32,                   // callee ident の列 (0-indexed)。SCIP join の鍵
    as_value: bool,             // `map(f)` のように **値として渡された関数参照** (呼ぶのは渡した先)
    in_macro: bool,             // parse できない macro body の字句走査で見つけた `ident(` (確定させない)
}

/// 1 関数ボディ内の呼び出し箇所を集める。
#[derive(Default)]
pub(crate) struct CallCollector {
    calls: Vec<RawCall>,
    /// このボディで束縛された名前 (let / 引数 / for / match / closure)。
    /// 値参照の候補から局所変数を落とすためのスコープ近似 — 無いと `len` / `path` / `id` のような
    /// ありふれた名前が method 名と衝突し、enchudb で候補が 14,738 行に膨れた。
    locals: std::collections::HashSet<String>,
}
impl CallCollector {
    /// 式に現れた **裸の path** を関数参照の候補として拾う (`map(f)` / `&f` / `Some(f)` /
    /// `f as fn(..)` / `S { field: f }` / `vec![f]` — 呼び出しの引数だけに限ると全部取り逃す)。
    /// これが無いと、高階関数経由でしか使われない定義が「誰からも呼ばれていない」に見える
    /// (実際 `sig_text` / `is_cfg_test` がそう見えていた — rustc は dead_code を出していないのに)。
    /// 裸の path は局所変数かもしれないので **確定はしない** (R_VALUE = 候補どまり)。さらに pass2 で
    /// 「その名前の **fn / method 定義** が index にある」物だけを残すので、変数名では膨らまない。
    /// 収集結果。値参照のうち **このボディの束縛名と同じ物は落とす** (局所変数の可能性が高い)。
    /// 巡回順に依存しないよう、全部集め終わってから 1 回で濾す。
    fn finish(self) -> Vec<RawCall> {
        let locals = self.locals;
        self.calls.into_iter().filter(|c| !c.as_value || !locals.contains(&c.name)).collect()
    }

    fn path_as_value(&mut self, p: &syn::ExprPath) {
        let segs = &p.path.segments;
        let Some(last) = segs.last() else { return };
        if segs.iter().any(|s| !s.arguments.is_none()) {
            return; // ジェネリク付き path (turbofish) は型の話なので見ない
        }
        let n = last.ident.to_string();
        if n == "self" || n == "Self" || n == "_" {
            return;
        }
        let qualifier = (segs.len() >= 2).then(|| segs[segs.len() - 2].ident.to_string());
        self.calls.push(RawCall {
            name: n,
            qualifier,
            is_method: false,
            self_recv: false,
            line: line_of(last.ident.span()),
            col: col_of(last.ident.span()),
            as_value: true,
            in_macro: false,
        });
    }
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
                    self_recv: false,
                    line: line_of(last.ident.span()),
                    col: col_of(last.ident.span()),
                    as_value: false,
                    in_macro: false,
                });
            }
        }
        // callee の path は「呼び出し」として既に記録したので、値参照として二重に数えない。
        // (func 以下だけ巡回を飛ばし、引数は通常どおり辿る)
        if !matches!(&*node.func, syn::Expr::Path(_)) {
            visit::visit_expr(self, &node.func);
        }
        for a in &node.args {
            visit::visit_expr(self, a);
        }
    }
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        // 受け手の型は syn だけでは不明 (→ 候補どまり)。**例外は `self.f()`** — 受け手は
        // 今いる impl の型なので、その container の同名 method に健全に確定できる。
        let self_recv = matches!(&*node.receiver, syn::Expr::Path(p) if p.path.is_ident("self"));
        self.calls.push(RawCall {
            name: node.method.to_string(),
            qualifier: None,
            is_method: true,
            self_recv,
            line: line_of(node.method.span()),
            col: col_of(node.method.span()),
            as_value: false,
            in_macro: false,
        });
        visit::visit_expr_method_call(self, node);
    }
    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        self.path_as_value(node);
        visit::visit_expr_path(self, node);
    }
    fn visit_pat_ident(&mut self, node: &'ast syn::PatIdent) {
        self.locals.insert(node.ident.to_string()); // let / 引数 / for / match / closure の束縛
        visit::visit_pat_ident(self, node);
    }
    /// macro 引数の中の呼び出し (`println!("{}", append_src(..))`) を拾う。
    /// token stream は AST に現れないので、ここを見ないと **call graph から丸ごと落ちる** —
    /// 「確実 + 候補 + 別 sym の合計で全 caller を掴める」という約束が破れていた
    /// (この repo の `append_src` / `role_name` が「誰からも呼ばれていない」に見えていた)。
    /// 式として parse できる macro (println! / format! / write! / assert! / vec! …) だけを通す。
    /// span は元 file のものがそのまま残るので、行・列も通常の呼び出しと同じ精度。
    /// parse できない DSL macro (quote! / matches! のパターン部など) は**黙って諦める** —
    /// token を舐めて `ident (` を拾うと pattern を呼び出しと誤認し、「確実 = 誤りなし」が崩れるため。
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        use syn::punctuated::Punctuated;
        if let Ok(args) = node.parse_body_with(Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated) {
            // parse した式は所有物 ('ast ではない) なので、別 collector で回して結果だけ貰う。
            let mut sub = CallCollector::default();
            for e in &args {
                visit::visit_expr(&mut sub, e);
            }
            self.calls.append(&mut sub.calls);
        } else {
            // 式として読めない DSL macro は字句で拾う (候補どまり)。
            scan_tokens_for_calls(node.tokens.clone(), &mut self.calls);
        }
        visit::visit_macro(self, node);
    }
}

/// pass1 で作る in-memory シンボル表の 1 定義。名前解決の突き合わせ先。
pub(crate) struct SymDef {
    eid: EntityId,
    kind: u32,         // method 呼び (x.foo()) は method 定義のみを対象にするため
    container: String, // impl 型 (method) / "" (free fn)。`Type::` 修飾の突き合わせ用
    module: String,    // "a::b"。`mod::` 修飾の突き合わせ用
    crate_: String,    // 定義元 crate。`mycrate::fn` / `enchudb_schema::foo` の突き合わせ用
}

/// Cargo の crate 名は `-`、Rust path は `_`。突き合わせ前に正規化する。
fn crate_key(s: &str) -> String {
    s.replace('-', "_")
}

/// pass2 まで持ち越す 1 呼び出し箇所 (caller は確定、callee はこれから解決)。
pub(crate) struct CallSite {
    pub(crate) caller: EntityId,
    pub(crate) caller_container: String, // `Self::` 修飾を現在の impl 型に解決するため
    pub(crate) file: EntityId,
    pub(crate) rel_path: String, // SCIP join の鍵 (index root からの相対)
    pub(crate) name: String,
    pub(crate) qualifier: Option<String>,
    pub(crate) is_method: bool,
    pub(crate) self_recv: bool,
    pub(crate) as_value: bool, // 値として渡された関数参照 (`map(f)`)
    pub(crate) in_macro: bool, // 字句走査で見つけた macro body 内の呼び出し
    pub(crate) line: u32, // callee ident 行 (1-indexed)
    pub(crate) col: u32,  // callee ident 列 (0-indexed)
}

/// 1 つの `impl Trait for Type`。go-to-implementation 用の edge。
pub(crate) struct ImplEdge {
    pub(crate) trait_name: String,
    pub(crate) type_name: String,
    pub(crate) file: EntityId,
    pub(crate) line: u32,
}

/// pass1/pass2 をまたいで貯める累積器 (ファイル横断)。
#[derive(Default)]
pub(crate) struct Acc {
    pub(crate) defs: HashMap<String, Vec<SymDef>>, // name → 定義群 (syn 解決用)
    pub(crate) pending: Vec<CallSite>,             // 未解決 call-site
    pub(crate) impls: Vec<ImplEdge>,               // impl Trait for Type edge
    pub(crate) scip: Option<Scip>,                 // Some なら SCIP 位置 join で正確解決
    pub(crate) sym_by_symbol: HashMap<String, EntityId>, // SCIP symbol 文字列 → 自 index の sym eid (0 = 複数定義に衝突)
    pub(crate) file_by_rel: HashMap<String, EntityId>,   // rel_path → file eid (SCIP ref-ingest 用)
    crate_cache: HashMap<PathBuf, String>,    // file の親 dir → crate 名 (crate_of の memo)
}

/// 木を歩く間の可変状態 (worker 側。db にも eid にも触らない)。
pub(crate) struct Ctx {
    module: Vec<String>,
    in_test: bool,
    container: String,     // 現在の impl 型 (method の所属)
    in_trait_impl: bool,   // `impl Trait for T` の中か (= trait 経由で呼ばれ得る method)
}

/// 1 ファイルから取り出した facts (eid 未割当の中間表現)。parse した thread で行/列・sig・doc まで
/// 確定させる — proc-macro2 の span-locations は thread-local なので、span を別 thread に持ち出せない。
/// 挿入 (eid 採番) は main thread が walk 順どおりに行う (insert_file_facts)。
pub(crate) struct FileFacts {
    pub(crate) loc: u32,
    pub(crate) hash: u32,
    pub(crate) syms: Vec<RawSym>,
    pub(crate) impls: Vec<RawImpl>,
    /// item 直下のマクロ (`criterion_group!(benches, bench_tie, …)`) の中の参照。
    /// 関数の中ではないので caller となる sym が無い = caller 無しの call 行として持つ。
    pub(crate) item_calls: Vec<RawCall>,
}

/// 1 定義 + その本体の呼び出し箇所。
pub(crate) struct RawSym {
    pub(crate) name: String,
    pub(crate) kind: u32,
    pub(crate) vis: u32,
    pub(crate) is_async: bool,
    pub(crate) is_test: bool,
    pub(crate) trait_impl: bool, // trait 実装の method (呼び出し側に名前が出ないので「未使用」に見える)
    pub(crate) module: String,
    pub(crate) container: String,
    pub(crate) line: u32,
    pub(crate) col: u32,
    pub(crate) end_line: u32,
    pub(crate) sig: String,
    pub(crate) doc: String,
    pub(crate) attrs: String, // 正規化済み属性 (`allow(dead_code) inline`)。facet `attr:` の材料
    pub(crate) calls: Vec<RawCall>,
}

/// `impl Trait for Type` (file eid は挿入時に付く)。
pub(crate) struct RawImpl {
    trait_name: String,
    type_name: String,
    line: u32,
}

/// fn シグネチャを 1 行文字列に (hover 相当)。token stream の機械的な空白を詰める。
pub(crate) fn sig_text(sig: &syn::Signature) -> String {
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
/// 1 定義を facts に積む (worker 側)。本体があれば呼び出し箇所もここで拾う (pass2 へ持ち越す材料)。
pub(crate) fn record_symbol(
    ctx: &mut Ctx,
    out: &mut FileFacts,
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
    attrs: &[syn::Attribute], // doc も属性もここから取る (呼び側の重複を無くす)
) {
    let mut calls = Vec::new();
    if let Some(b) = body {
        let mut cc = CallCollector::default();
        // 引数名は signature 側にあるので、body だけ歩くと束縛が漏れる。
        // (tokio の `registration` のような仮引数が「同名関数への参照」に化けていた)
        if let Some(s) = sig {
            for a in &s.inputs {
                if let syn::FnArg::Typed(t) = a {
                    cc.visit_pat(&t.pat);
                }
            }
        }
        cc.visit_block(b);
        calls = cc.finish();
    }
    out.syms.push(RawSym {
        name: name.to_string(),
        kind,
        vis,
        is_async,
        is_test,
        trait_impl: ctx.in_trait_impl,
        module: ctx.module.join("::"),
        container: ctx.container.clone(),
        line,
        col,
        end_line,
        sig: sig.map(sig_text).unwrap_or_default(),
        doc: first_doc_line(attrs),
        attrs: attrs_text(attrs),
        calls,
    });
}

/// facts を db に挿入して eid を採番する (main thread、walk 順)。SCIP があれば定義位置から
/// グローバル symbol 文字列を引いて焼き込み、call-site は caller eid 付きで pass2 へ持ち越す。
pub(crate) fn insert_file_facts(file_t: &Table, sym_t: &Table, acc: &mut Acc, root: &str, path_s: &str, facts: FileFacts) {
    let crate_name = crate_of(path_s, &mut acc.crate_cache);
    let file_eid = file_t
        .insert()
        .set("path", path_s)
        .set("crate_", crate_name.clone())
        .set("lang", LANG_RUST)
        .set("loc", facts.loc)
        .set("hash", facts.hash)
        .commit()
        .unwrap();
    let rel_path = rel_of(path_s, root);
    acc.file_by_rel.insert(rel_path.clone(), file_eid);
    for s in facts.syms {
        let symbol = acc
            .scip
            .as_ref()
            .and_then(|sc| sc.symbol_at(&rel_path, s.line, s.col))
            .map(str::to_string)
            .unwrap_or_default();
        let sym_eid = sym_t
            .insert()
            .set("name", s.name.as_str())
            .set("kind", s.kind)
            .set("vis", s.vis)
            .set("is_async", s.is_async as u32)
            .set("is_test", s.is_test as u32)
        .set("trait_impl", s.trait_impl as u32)
            .set("file", Value::Ref(file_eid))
            .set("module", s.module.clone())
            .set("crate_", crate_name.clone())
            .set("container", s.container.clone())
            .set("symbol", symbol.as_str())
            .set("sig", s.sig.as_str())
            .set("doc", s.doc.as_str())
        .set("attrs", s.attrs.as_str())
            .set("line", s.line)
            .set("end_line", s.end_line)
            .commit()
            .unwrap();
        // 名前解決の突き合わせ先として登録 (syn 用)。
        acc.defs.entry(s.name).or_default().push(SymDef { eid: sym_eid, kind: s.kind, container: s.container.clone(), module: s.module, crate_: crate_name.clone() });
        // SCIP symbol → 自 index の eid (call の正確解決に使う)。
        // **衝突したら 0 (曖昧) にする**: RA の symbol 文字列は test / example / bench の各ターゲットで
        // 同じになることがあり (enchudb には同名 `cleanup` が 108 個)、上書きすると
        // 全ターゲットの呼び出しが最後の 1 定義に集まって「確実 caller 275 件」のような嘘になる。
        // 曖昧なら SCIP を使わず syn 側の規準に落とす (同名多数 → 候補どまり) = 誤りを出さない。
        if !symbol.is_empty() {
            acc.sym_by_symbol
                .entry(symbol)
                .and_modify(|e| *e = AMBIGUOUS_SYMBOL)
                .or_insert(sym_eid);
        }
        // 呼び出し箇所は全 sym 確定後に解決するので pass2 へ持ち越す。
        for rc in s.calls {
            acc.pending.push(CallSite {
                caller: sym_eid,
                caller_container: s.container.clone(),
                file: file_eid,
                rel_path: rel_path.clone(),
                name: rc.name,
                qualifier: rc.qualifier,
                is_method: rc.is_method,
                self_recv: rc.self_recv,
                as_value: rc.as_value,
                in_macro: rc.in_macro,
                line: rc.line,
                col: rc.col,
            });
        }
    }
    // item 直下の参照は caller sym を持たない (関数の外)。位置だけで十分意味がある
    // (「使われているか」に答える + `callers` に file:line で出す)。
    for rc in facts.item_calls {
        acc.pending.push(CallSite {
            caller: 0, // = 無し。do_insert_call が caller 列を set しない
            caller_container: String::new(),
            file: file_eid,
            rel_path: rel_path.clone(),
            name: rc.name,
            qualifier: rc.qualifier,
            is_method: rc.is_method,
            self_recv: rc.self_recv,
            as_value: rc.as_value,
            in_macro: rc.in_macro,
            line: rc.line,
            col: rc.col,
        });
    }
    for im in facts.impls {
        acc.impls.push(ImplEdge { trait_name: im.trait_name, type_name: im.type_name, file: file_eid, line: im.line });
    }
}

/// parse できない macro body (`proptest! { fn f(x in strategy) { helper(); } }` のような DSL) を
/// **字句で**走査し、`ident(` を呼び出し候補として拾う。式としても item としても parse できない形が
/// 実在する (proptest の `x in strategy` は Rust の文法ではない) ので、ここだけは token に落ちる。
/// 見つけた物は **確定させない** (R_MACRO = 候補どまり) — pattern や型名を呼び出しと誤認しても
/// 「確実 = 誤りなし」を壊さないため。定義表に無い名前は pass2 で捨てる。
pub(crate) fn scan_tokens_for_calls(ts: proc_macro2::TokenStream, out: &mut Vec<RawCall>) {
    use proc_macro2::TokenTree;
    const KEYWORDS: &[&str] = &[
        "if", "match", "while", "for", "loop", "fn", "return", "let", "in", "as", "impl", "mod", "use",
        "pub", "struct", "enum", "trait", "self", "Self", "move", "where", "const", "static", "unsafe", "dyn",
    ];
    let (mut prev, mut bang) = (None::<proc_macro2::Ident>, false);
    for t in ts {
        match t {
            TokenTree::Ident(i) => {
                prev = Some(i);
                bang = false;
            }
            TokenTree::Punct(p) => {
                if p.as_char() == '!' {
                    bang = true; // `name!(...)` は macro 呼び出しであって関数呼び出しではない
                } else if p.as_char() != ':' {
                    prev = None; // `a::b(` の `::` は path なので prev を消さない
                    bang = false;
                }
            }
            TokenTree::Group(g) => {
                if let Some(i) = prev.take()
                    && !bang
                    && g.delimiter() == proc_macro2::Delimiter::Parenthesis
                {
                    let name = i.to_string();
                    if !KEYWORDS.contains(&name.as_str()) {
                        out.push(RawCall {
                            name,
                            qualifier: None,
                            is_method: false,
                            self_recv: false,
                            line: line_of(i.span()),
                            col: col_of(i.span()),
                            as_value: false,
                            in_macro: true,
                        });
                    }
                }
                bang = false;
                scan_tokens_for_calls(g.stream(), out);
            }
            TokenTree::Literal(_) => {
                prev = None;
                bang = false;
            }
        }
    }
}

/// 属性を facet 可能な 1 行に正規化: `#[allow(dead_code)] #[inline]` → `allow(dead_code) inline`。
/// doc コメントは除く (doc 列が持つ)。空白を畳むので `cfg(feature = "x")` は `cfg(feature="x")` になる。
/// **特定の属性を特別扱いしない** — dead_code の裏取りにも deprecated の利用調査にも同じ 1 本で効く。
pub(crate) fn attrs_text(attrs: &[syn::Attribute]) -> String {
    let mut v: Vec<String> = Vec::new();
    for a in attrs {
        if a.path().is_ident("doc") {
            continue;
        }
        let raw = a.meta.to_token_stream().to_string();
        v.push(raw.split_whitespace().collect::<Vec<_>>().join(""));
    }
    let joined = v.join(" ");
    joined.chars().take(ATTRS_MAX).collect()
}

/// attrs 列の上限 (vocab を無闇に膨らませない。derive の長い羅列は切って構わない)。
pub(crate) const ATTRS_MAX: usize = 200;

/// attrs の doc コメント 1 行目 (`///` 由来の `#[doc = "…"]` の最初の非空行)。無ければ ""。
/// def/outline で sig と並べて出す用なので 100 char で切る。
pub(crate) fn first_doc_line(attrs: &[syn::Attribute]) -> String {
    for a in attrs {
        if !a.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &a.meta
            && let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
                let t = s.value().trim().to_string();
                if !t.is_empty() {
                    return t.chars().take(100).collect();
                }
            }
    }
    String::new()
}

pub(crate) fn walk_items(items: &[syn::Item], ctx: &mut Ctx, out: &mut FileFacts) {
    for it in items {
        walk_item(it, ctx, out);
    }
}

pub(crate) fn walk_item(it: &syn::Item, ctx: &mut Ctx, out: &mut FileFacts) {
    match it {
        syn::Item::Fn(f) => {
            let is_test = ctx.in_test || is_test_attrs(&f.attrs);
            record_symbol(
                ctx,
                out,
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
                &f.attrs,
            );
        }
        syn::Item::Struct(s) => record_symbol(
            ctx, out, &s.ident.to_string(), K_STRUCT,
            classify_vis(&s.vis), false, ctx.in_test, line_of(s.ident.span()), col_of(s.ident.span()), end_line_of(s), None, None,
            &s.attrs,
        ),
        syn::Item::Enum(e) => record_symbol(
            ctx, out, &e.ident.to_string(), K_ENUM,
            classify_vis(&e.vis), false, ctx.in_test, line_of(e.ident.span()), col_of(e.ident.span()), end_line_of(e), None, None,
            &e.attrs,
        ),
        syn::Item::Const(c) => record_symbol(
            ctx, out, &c.ident.to_string(), K_CONST,
            classify_vis(&c.vis), false, ctx.in_test, line_of(c.ident.span()), col_of(c.ident.span()), end_line_of(c), None, None,
            &c.attrs,
        ),
        syn::Item::Trait(t) => {
            record_symbol(
                ctx, out, &t.ident.to_string(), K_TRAIT,
                classify_vis(&t.vis), false, ctx.in_test, line_of(t.ident.span()), col_of(t.ident.span()), end_line_of(t), None, None,
                &t.attrs,
            );
            // trait method の container は trait 名 (= `callers put BlobStore` で引けるように)。
            let prev = std::mem::replace(&mut ctx.container, t.ident.to_string());
            for ti in &t.items {
                if let syn::TraitItem::Fn(m) = ti {
                    let is_test = ctx.in_test || is_test_attrs(&m.attrs);
                    record_symbol(
                        ctx, out, &m.sig.ident.to_string(), K_METHOD,
                        V_PUB, m.sig.asyncness.is_some(), is_test,
                        line_of(m.sig.ident.span()), col_of(m.sig.ident.span()), end_line_of(m), Some(&m.sig), m.default.as_ref(),
                        &m.attrs,
                    );
                }
            }
            ctx.container = prev;
        }
        syn::Item::Impl(i) => {
            let type_name = clean_type(&i.self_ty);
            // `impl Trait for Type` なら impl edge を記録 (go-to-implementation 用)。
            // trait 名 = path 末尾 seg、line も同 seg の Ident span (Type の span は trait 不要で避ける)。
            if let Some((_, tpath, _)) = &i.trait_
                && let Some(seg) = tpath.segments.last() {
                    out.impls.push(RawImpl {
                        trait_name: seg.ident.to_string(),
                        type_name: type_name.clone(),
                        line: line_of(seg.ident.span()),
                    });
                }
            let prev = std::mem::replace(&mut ctx.container, type_name);
            // trait 実装の method は呼び出し側に名前が出ない (trait 経由 / dyn) ため、
            // 「誰からも呼ばれていない」に構造的に見える。区別できるよう印を付ける。
            let prev_ti = std::mem::replace(&mut ctx.in_trait_impl, i.trait_.is_some());
            for ii in &i.items {
                if let syn::ImplItem::Fn(m) = ii {
                    let is_test = ctx.in_test || is_test_attrs(&m.attrs);
                    record_symbol(
                        ctx, out, &m.sig.ident.to_string(), K_METHOD,
                        classify_vis(&m.vis), m.sig.asyncness.is_some(), is_test,
                        line_of(m.sig.ident.span()), col_of(m.sig.ident.span()), end_line_of(m), Some(&m.sig), Some(&m.block),
                        &m.attrs,
                    );
                }
            }
            ctx.container = prev;
            ctx.in_trait_impl = prev_ti;
        }
        // item 直下のマクロ: `criterion_group!(benches, bench_tie, …)` のように **関数の外**で
        // 定義を参照する形。CallCollector は関数本体しか歩かないので、ここを見ないと
        // bench 関数群が丸ごと「誰からも呼ばれていない」に見える (実際そう見えていた)。
        syn::Item::Macro(m) => {
            use syn::punctuated::Punctuated;
            if let Ok(args) = m.mac.parse_body_with(Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated) {
                let mut cc = CallCollector::default();
                for e in &args {
                    visit::visit_expr(&mut cc, e);
                }
                out.item_calls.append(&mut cc.finish());
            } else {
                scan_tokens_for_calls(m.mac.tokens.clone(), &mut out.item_calls);
            }
        }
        syn::Item::Mod(m) => {
            if let Some((_, inner)) = &m.content {
                let test = ctx.in_test || m.attrs.iter().any(is_cfg_test);
                ctx.module.push(m.ident.to_string());
                let prev = std::mem::replace(&mut ctx.in_test, test);
                walk_items(inner, ctx, out);
                ctx.in_test = prev;
                ctx.module.pop();
            }
        }
        _ => {}
    }
}

// ─────────────────────────── 名前解決 (pass2) ───────────────────────────

/// 1 呼び出し箇所を定義表に突き合わせて (解決先 eid, 信頼度) を返す。推測はしない。
pub(crate) fn resolve_call(cs: &CallSite, defs: &HashMap<String, Vec<SymDef>>) -> (Option<EntityId>, u32) {
    let Some(all_cands) = defs.get(&cs.name) else {
        return (None, R_EXTERNAL); // 呼べる同名定義が index に無い = std / 依存 crate (解決不能)
    };
    if cs.in_macro {
        // 字句走査の当て推量なので確定させない。呼べる物の名前でなければ記録もしない。
        if !all_cands.iter().any(|d| d.kind == K_FN || d.kind == K_METHOD) {
            return (None, R_UNRESOLVED);
        }
        return (None, R_MACRO);
    }
    if cs.as_value {
        // 値参照の裸 path は局所変数と見分けが付かない → **確定させない** (候補どまり)。
        // 呼べる物 (fn / method) の名前でなければ、変数か型の参照なので記録しない。
        if !all_cands.iter().any(|d| d.kind == K_FN || d.kind == K_METHOD) {
            return (None, R_UNRESOLVED); // 呼び側で捨てられる
        }
        return (None, R_VALUE);
    }
    // method 呼び (x.foo()) の対象は method 定義のみ。free fn `foo()` への誤解決を防ぐ。
    let cands: Vec<&SymDef> = if cs.is_method {
        all_cands.iter().filter(|d| d.kind == K_METHOD).collect()
    } else {
        all_cands.iter().collect()
    };
    if cands.is_empty() {
        return (None, R_EXTERNAL); // `x.len()` — 同名の method 定義が repo に無い = 外部型の method
    }
    // `super::` / `crate::` / `self::` は **型修飾ではなく module 相対の道順**。型として突き合わせると
    // 必ず外れるので、修飾なしと同じ扱い (同名が一意なら解決) にする。テスト module の
    // `super::foo()` が 1 件も確定しなかったのはこれ (Rust では最も普通の書き方の 1 つ)。
    let qualifier = cs.qualifier.as_deref().filter(|q| !matches!(*q, "super" | "crate" | "self"));
    // 修飾あり: `Type::` / `Self::` / `mod::` で 1 つに絞れるかを試す。
    if let Some(q) = qualifier {
        // `Self::` は現在の impl 型に読み替える。
        let target = if q == "Self" { cs.caller_container.as_str() } else { q };
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
        // (c) crate 名一致 = `mycrate::fn` / `enchudb_schema::foo`。workspace の他 crate を
        // 名前で呼ぶ形は多 crate repo では普通なのに、ここが無いと全部「外部」に落ちていた。
        let by_crate: Vec<&SymDef> =
            cands.iter().copied().filter(|d| crate_key(&d.crate_) == crate_key(target)).collect();
        if by_crate.len() == 1 {
            return (Some(by_crate[0].eid), R_QUALIFIED);
        }
        // 修飾があるのに index 内で一致しない = 外部型/モジュール。単純名 fallback はしない
        // (`HashMap::new` を自前の `Foo::new` に誤解決しないため)。
        return (None, R_EXTERNAL);
    }
    // 修飾なし の method 呼び (`x.f()`): 受け手の型が分からないので、同名 method が repo に
    // 1 つしか無くても **確定させない**。`.next()` / `.len()` のように std/dep 側の同名 method を
    // 自前の定義に誤確定してしまい、「確実 = 誤りなし」が崩れるため (実例: `callers next` が
    // `t.split(:).next()` を `Lcg::next` の確実 caller として並べていた)。症状は syn 層の確定率が
    // bake 後より高くなること — kenning で 88.3% (syn) > 85.4% (bake)、修正後は 82.0% / 82.1% で一致。
    // ただし `self.f()` は受け手 = 今いる impl の型と分かるので確定できる。
    if cs.is_method {
        if cs.self_recv {
            let by_self: Vec<&SymDef> =
                cands.iter().copied().filter(|d| d.container == cs.caller_container).collect();
            if by_self.len() == 1 {
                return (Some(by_self[0].eid), R_UNIQUE);
            }
        }
        return (None, R_METHOD); // 候補どまり (名前一致は残るので `callers` には出る)
    }
    // 修飾なしの関数呼び: 同名がちょうど 1 つなら一意解決、複数なら諦める。
    if cands.len() == 1 {
        (Some(cands[0].eid), R_UNIQUE)
    } else {
        (None, R_AMBIG)
    }
}

/// 解決済み (target, res) で call 行を挿入。解決した call だけ callee_sym を set。
pub(crate) fn do_insert_call(call_t: &Table, cs: &CallSite, target: Option<EntityId>, res: u32) {
    let mut ins = call_t
        .insert()
        .set("callee", cs.name.as_str())
        .set("res", res)
        .set("qual", cs.qualifier.as_deref().unwrap_or(""))
        .set("is_method", if cs.self_recv { M_SELF } else if cs.is_method { M_RECV } else { M_NONE })
        .set("file", Value::Ref(cs.file))
        .set("line", cs.line);
    if cs.caller != 0 {
        ins = ins.set("caller", Value::Ref(cs.caller)); // 0 = item 直下 (関数の外) → 未 set
    }
    if let Some(eid) = target {
        ins = ins.set("callee_sym", Value::Ref(eid));
    }
    ins.commit().unwrap();
}

/// syn ヒューリスティックで callee を解決して挿入。res を返す。
pub(crate) fn insert_call(call_t: &Table, cs: &CallSite, defs: &HashMap<String, Vec<SymDef>>) -> u32 {
    let (target, res) = resolve_call(cs, defs);
    if (cs.as_value || cs.in_macro) && matches!(res, R_UNRESOLVED | R_EXTERNAL) {
        return res; // 定義表に無い名前 = 局所変数 / 外部。記録しない (index 側と同じ判断)
    }
    do_insert_call(call_t, cs, target, res);
    res
}

/// 全 sym 行を走査して `name → [定義]` 表を作る (再パース不要)。増分の再解決で使う。
pub(crate) fn build_defs_from_table(sym_t: &Table) -> HashMap<String, Vec<SymDef>> {
    let mut defs: HashMap<String, Vec<SymDef>> = HashMap::new();
    for e in sym_t.all().find().unwrap() {
        let er = sym_t.entity(e);
        defs.entry(txt(er.get("name"))).or_default().push(SymDef {
            eid: e,
            kind: num(er.get("kind")),
            container: txt(er.get("container")),
            module: txt(er.get("module")),
            crate_: txt(er.get("crate_")),
        });
    }
    defs
}

/// 1 ファイルの sym / call / impl / file 行を全消去。消えた sym の名前を `affected` に集める
/// (他ファイルからの incoming edge を後で再解決するため)。impl_t は旧 index で無いこともある。
pub(crate) fn purge_file(sym_t: &Table, call_t: &Table, file_t: &Table, impl_t: Option<&Table>, file_eid: EntityId, affected: &mut HashSet<String>) {
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
pub(crate) fn index_one_file(file_t: &Table, sym_t: &Table, acc: &mut Acc, root: &str, path_s: &str, src: &str) -> bool {
    let Some(facts) = extract_file(src) else { return false };
    insert_file_facts(file_t, sym_t, acc, root, path_s, facts);
    true
}

/// 1 ファイルを parse して facts を取り出す (thread-safe: db にも acc にも触らない)。parse 失敗は None。
pub(crate) fn extract_file(src: &str) -> Option<FileFacts> {
    let file = syn::parse_file(src).ok()?;
    let mut out = FileFacts { loc: src.lines().count() as u32, hash: hash_u32(src), syms: Vec::new(), impls: Vec::new(), item_calls: Vec::new() };
    // file 冒頭の inner attribute (`#![cfg(test)]`) は file 全体に効く。per-file parse では親の
    // `#[cfg(test)] mod x;` が見えないので、これを見ないと test 専用 file の helper が
    // 「test でない symbol」として出てしまう (`tests <name>` と `search test:1` が取りこぼす)。
    let mut ctx = Ctx { module: Vec::new(), in_test: file.attrs.iter().any(is_cfg_test), container: String::new(), in_trait_impl: false };
    walk_items(&file.items, &mut ctx, &mut out);
    Some(out)
}

/// full index の pass1 で 1 度に並列 parse する file 数。facts の常駐をこの単位に抑える
/// (巨大 repo で全 file の facts を同時に抱えない)。
pub(crate) const PARSE_BATCH: usize = 512;

/// `paths` を読んで parse し facts を返す (順序は `paths` と同じ)。parse は syn で tokio 725 files に
/// 逐次 290ms / 12 thread 72ms。読めない・parse できない file は None。
pub(crate) fn extract_parallel(paths: &[PathBuf]) -> Vec<Option<FileFacts>> {
    par_map(paths, |p| std::fs::read_to_string(p).ok().and_then(|s| extract_file(&s)))
}

/// `items` を core 数で等分して scoped thread に撒き、結果を **入力順** で返す (出力の決定性は
/// 呼び側が気にしなくていい)。parse (extract_parallel) と `text` の file 読みで共用。
pub(crate) fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, items.len().max(1));
    let chunk = items.len().div_ceil(n).max(1);
    std::thread::scope(|sc| {
        let hs: Vec<_> = items.chunks(chunk).map(|c| sc.spawn(|| c.iter().map(&f).collect::<Vec<_>>())).collect();
        hs.into_iter().flat_map(|h| h.join().expect("worker panicked")).collect()
    })
}

/// index 対象の walk 規約 (.rs / それ以外の共通土台)。**ripgrep と同じ規約**:
/// .gitignore / .ignore / 隠し dir を尊重する。生成物の展開先や vendor 複製が index に入ると、
/// 本体とほぼ同一のコピーが `def` の重複シンボル・`text` の重複ヒットになり、しかもコピーは
/// 展開時点のスナップショット = 古い (#12)。
///
/// target/ だけは gitignore に頼らず**明示枝刈り** (ignore していない repo 対策)。
/// パス文字列の後段フィルタでなく filter_entry なのは、target/ 数十万ファイルを列挙してから捨てると
/// staleness stat-walk が毎クエリ秒単位になるため (filter_entry なら降りない)。
///
/// `KENNING_NO_IGNORE=1` で ignore 規則だけ無効化 — gitignore 済みだが実際に compile される
/// 生成 .rs を持つ repo の逃げ道 (hidden / target の枝刈りは残る)。
pub(crate) fn index_walk(dir: &str) -> ignore::WalkBuilder {
    let respect = std::env::var_os("KENNING_NO_IGNORE").is_none();
    let mut b = ignore::WalkBuilder::new(dir);
    b.hidden(true) // .git / .claude / .turbo … 正規の source は隠し dir に住まない
        .ignore(respect)
        .git_ignore(respect)
        .git_global(respect)
        .git_exclude(respect)
        .parents(respect)
        .filter_entry(|e| e.depth() == 0 || (!is_always_pruned(e.file_name()) && !is_kenning_artifact(e.file_name())));
    b
}

/// gitignore に関係なく常に降りない dir。`target/` (cargo 成果物) と `node_modules/` (ravn の mcp/ で
/// 未 ignore の 3,420 file が text として索引され、full index 1.8s / `text` 110ms の主因だった)。
/// Rust の source がここに住むことは無い。
pub(crate) fn is_always_pruned(name: &std::ffi::OsStr) -> bool {
    name == "target" || name == "node_modules"
}

/// kenning 自身が repo 内に置き得る成果物 (明示 `--db` で repo 内に index を作った時の db directory /
/// index lock / 作りかけ)。index 対象に混ぜると sidecar の JSON が `text` に出てくるので枝刈りする。
pub(crate) fn is_kenning_artifact(name: &std::ffi::OsStr) -> bool {
    let n = name.to_string_lossy();
    n.ends_with(".db") || n.ends_with(".index.lock") || n.contains(".db.tmp-") || n.contains(".db.old-")
}

/// dir 以下の index 対象 .rs を列挙 (walk 規約は index_walk)。
pub(crate) fn rust_files(dir: &str) -> impl Iterator<Item = std::path::PathBuf> {
    walk_index_set(dir).rs.into_iter()
}

/// 1 回の walk で拾った index 対象。`.rs` / それ以外のテキスト / 通過した dir (root 含む)。
#[derive(Default)]
pub(crate) struct WalkSet {
    pub(crate) rs: Vec<PathBuf>,
    pub(crate) text: Vec<PathBuf>,
    pub(crate) dirs: Vec<PathBuf>,
}

/// index 対象を **1 回の walk** で全部集める。walk は .gitignore 解釈込みで 770 files に ~4.5ms かかる
/// ので、rust_files と text_files を別々に呼ぶと倍払う (鮮度判定 + 増分 update で 4 回、full index で
/// 4 回、回していた)。dir は鮮度判定の dir ゲート (fs_gate) が「file の増減が無い」と言うための材料。
pub(crate) fn walk_index_set(dir: &str) -> WalkSet {
    let mut w = WalkSet::default();
    for e in index_walk(dir).build().filter_map(Result::ok) {
        let Some(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            w.dirs.push(e.path().to_path_buf());
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        if e.path().extension().is_some_and(|x| x == "rs") {
            w.rs.push(e.path().to_path_buf());
        } else if !GENERATED_FILES.contains(&e.file_name().to_string_lossy().as_ref())
            && e.metadata().is_ok_and(|m| m.len() <= TEXT_MAX_BYTES)
        {
            w.text.push(e.path().to_path_buf());
        }
    }
    w
}

/// 拡張子 → lang。抽出関数を持たない形式は LANG_TEXT (注釈なしで索引されるだけ)。
pub(crate) fn lang_of(path: &std::path::Path) -> u32 {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => LANG_RUST,
        "md" | "markdown" => LANG_MD,
        "toml" => LANG_TOML,
        "yml" | "yaml" => LANG_YAML,
        _ => LANG_TEXT,
    }
}

/// NUL byte を含むか (git と同じ binary 判定規約)。先頭 BINARY_SNIFF bytes だけ嗅ぐ。
pub(crate) fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF).any(|&b| b == 0)
}

/// dir 以下の **.rs 以外の索引対象テキスト**を列挙。形式は問わない (binary とサイズ超過だけ落とす)。
/// walk 規約は rust_files と共通 (index_walk) — 片方だけ gitignore を尊重すると、同じ
/// `kenning text` の中で vendor/*.md は落ちるのに vendor/*.rs は出る、という不整合になる (#12)。
#[cfg(test)]
pub(crate) fn text_files(dir: &str) -> impl Iterator<Item = std::path::PathBuf> {
    walk_index_set(dir).text.into_iter()
}

/// text index 用の読み込み。binary / 非 UTF-8 / サイズ超過は None = 索引しない (嘘を出さない)。
pub(crate) fn read_text_file(p: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(p).ok()?;
    if bytes.len() as u64 > TEXT_MAX_BYTES || is_probably_binary(&bytes) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// 非 Rust テキストを file 表に載せる。sym は持たない — `text` の全文検索対象になるだけで、
/// 本文は query 時に読む (索引に持つのは path/hash/loc のみ = 肥大しない)。
pub(crate) fn index_text_file(file_t: &Table, acc: &mut Acc, path_s: &str, src: &str) {
    let crate_name = crate_of(path_s, &mut acc.crate_cache);
    file_t
        .insert()
        .set("path", path_s)
        .set("crate_", crate_name)
        .set("lang", lang_of(std::path::Path::new(path_s)))
        .set("loc", src.lines().count() as u32)
        .set("hash", hash_u32(src))
        .commit()
        .unwrap();
}

/// 非 Rust テキストの container 注釈を「(行, container)」の昇順で返す (`text` が hit 行から逆引き)。
/// .rs の enclosing symbol と同じ使い勝手を、形式ごとの小さい抽出で埋める。
/// 抽出関数を持たない形式は空 = 注釈なしで通す。**索引ではなく query 時に本文から作る**ので
/// 形式を足しても index 版は動かない。
pub(crate) fn text_containers(lang: u32, src: &str) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    // (深さ, 見出し/キー) のスタック。md は heading level、yaml は indent 幅を深さに使う。
    let mut stack: Vec<(usize, String)> = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let ln = (i + 1) as u32;
        match lang {
            LANG_MD => {
                let t = raw.trim_start();
                let level = t.bytes().take_while(|&b| b == b'#').count();
                if level == 0 || level > 6 || !t[level..].starts_with(' ') {
                    continue;
                }
                let title = t[level..].trim().trim_end_matches('#').trim().to_string();
                if title.is_empty() {
                    continue;
                }
                stack.retain(|(d, _)| *d < level);
                stack.push((level, title));
                out.push((ln, stack.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join(" > ")));
            }
            LANG_TOML => {
                let t = raw.trim();
                if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    let name = inner.trim_start_matches('[').trim_end_matches(']').trim();
                    if !name.is_empty() {
                        out.push((ln, name.to_string()));
                    }
                }
            }
            LANG_YAML => {
                let indent = raw.len() - raw.trim_start().len();
                let t = raw.trim_start();
                if t.starts_with('#') || t.starts_with('-') {
                    continue;
                }
                let Some(key) = t.split(':').next().filter(|k| {
                    !k.is_empty()
                        && k.len() < t.len()
                        && k.chars().all(|c| c.is_alphanumeric() || "_-.".contains(c))
                }) else {
                    continue;
                };
                stack.retain(|(d, _)| *d < indent);
                stack.push((indent, key.to_string()));
                out.push((ln, stack.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join(".")));
            }
            _ => break, // 抽出関数なし = 注釈なし
        }
    }
    out
}
