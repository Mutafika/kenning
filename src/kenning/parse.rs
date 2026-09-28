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

/// method 呼び `x.f()` の受け手について、ソースに**書いてある**型の手掛かり。推定の材料であって
/// 確定ではない (解決時に「その型の同名 method がちょうど 1 つ」の時だけ確定する)。
#[derive(Clone, Default, Debug, PartialEq)]
pub(crate) struct Recv {
    /// 型名 (path 末尾 seg。`Self` は解決時に caller の impl 型へ)。空 = 手掛かり無し。
    pub(crate) ty: String,
    /// `let x = T::f(..)` 由来なら `f` (型は f の戻り値が `Self` / `T` の時だけ T と認める)。
    pub(crate) via_fn: String,
    /// `T::f(..)?` — 戻り値 `Result<Self, _>` / `Option<Self>` の中身を取る。
    pub(crate) via_try: bool,
    /// 型の出どころ (path の先頭 / `use` の先頭)。repo の外 (`std` / `walkdir`) なら同名の自前の型と
    /// 結び付けない (`walkdir::DirEntry` を自前の `DirEntry` に取り違えた実例)。空 = 手掛かり無し (ローカル)。
    pub(crate) root: String,
    /// 起点からの手順 (空白区切り): `f:<field>` / `m:<method>` / `?`。`self.cfg.searcher().run()` の受け手は
    /// 起点 Self + `f:cfg m:searcher`。解決時に 1 段ずつ型をたどる (1 段でも絞れなければ推定しない)。
    pub(crate) chain: String,
}

/// 連鎖の手順の上限 (長い連鎖ほど 1 段の取り違えが響くので、そこそこで諦める)。
const CHAIN_MAX: usize = 8;

/// 受け手が自由関数の戻り値だった印 (`Recv.ty` = 印 + 修飾ヒント。`time::sleep(..)` なら `fn:time`)。
/// 型名になり得ない文字を含むので、どの型名とも一致しない。
const FREE_FN_RECV: &str = "fn:";

/// 中身の method に自動参照外しで届く std の smart pointer と、1 引数で包む constructor。
const SMART_PTRS: &[&str] = &["Box", "Pin", "Arc", "Rc"];
const SMART_PTR_CTORS: &[&str] = &["new", "pin"];

/// smart pointer で包んだ受け手では、中身の method より先に当たり得る名前 (Pin の inherent method と、
/// Box / Pin / Arc / Rc が中身に応じて実装する std trait の method)。包んだ受け手のこの名前は推定しない。
const SMART_PTR_METHODS: &[&str] = &[
    "as_mut", "as_ref", "set", "get_mut", "get_ref", "into_inner", "into_ref", "get_unchecked_mut",
    "map_unchecked", "map_unchecked_mut", "as_deref", "as_deref_mut", "clone", "poll", "poll_next", "next",
    "next_back", "deref", "deref_mut", "fmt", "eq", "ne", "cmp", "partial_cmp", "hash", "borrow", "borrow_mut",
    "drop", "into_iter", "to_string", "to_owned", "into", "try_into", "call", "call_mut", "call_once",
];

/// 書いてある型 1 つ: 名前 (path 末尾、別名は元の名前) + 出どころ + 最初の型引数 (`Result<T, E>` の T)。
/// fn の戻り値と struct の field の型として保存し、連鎖をたどる時に使う。
#[derive(Clone, Default, Debug, PartialEq)]
pub(crate) struct TyRef {
    pub(crate) name: String,
    pub(crate) root: String,
    pub(crate) arg: String,
    pub(crate) arg_root: String,
}

/// `use` の引き方 (名前 → (出どころ, 元の名前))。関数の中の `use` を優先したい時は重ねて渡す。
pub(crate) type ImportLookup<'a> = &'a dyn Fn(&str) -> Option<(String, String)>;

/// 型 path の出どころ (`a::b::T` なら a を `use` で引き直す、単独 `T` は `use` の先頭、無ければ空 = ローカル)。
pub(crate) fn type_root_of(path: &syn::Path, imp: ImportLookup) -> String {
    let first = path.segments.first().map(|s| s.ident.to_string()).unwrap_or_default();
    if path.segments.len() == 1 {
        return imp(&first).map(|i| i.0).unwrap_or_default();
    }
    match first.as_str() {
        "crate" | "self" | "super" | "Self" => first,
        _ => imp(&first).map(|i| i.0).unwrap_or(first),
    }
}

/// 単独の型名が `use .. as 別名` の別名なら元の名前に戻す (定義の container は元の名前)。
pub(crate) fn unalias_of(path: &syn::Path, last: String, imp: ImportLookup) -> String {
    if path.segments.len() == 1
        && let Some((_, orig)) = imp(&last)
    {
        return orig;
    }
    last
}

/// 書いてある型 → TyRef。`&T` / `&mut T` / `T<..>` は T、型引数・`impl`/`dyn`・tuple 等は None。
pub(crate) fn ty_ref(ty: &syn::Type, imp: ImportLookup, generics: &HashSet<String>) -> Option<TyRef> {
    match ty {
        syn::Type::Reference(r) => ty_ref(&r.elem, imp, generics),
        syn::Type::Paren(p) => ty_ref(&p.elem, imp, generics),
        syn::Type::Group(g) => ty_ref(&g.elem, imp, generics),
        syn::Type::Path(p) if p.qself.is_none() => {
            let seg = p.path.segments.last()?;
            let last = seg.ident.to_string();
            if p.path.segments.len() == 1 && generics.contains(&last) {
                return None;
            }
            let arg = match &seg.arguments {
                syn::PathArguments::AngleBracketed(a) => a.args.iter().find_map(|g| match g {
                    syn::GenericArgument::Type(t) => ty_ref(t, imp, generics),
                    _ => None,
                }),
                _ => None,
            }
            .unwrap_or_default();
            Some(TyRef { name: unalias_of(&p.path, last, imp), root: type_root_of(&p.path, imp), arg: arg.name, arg_root: arg.root })
        }
        _ => None,
    }
}

/// 引数を式としてそのまま評価する std のマクロ (受け手の型の推定をそのまま効かせてよい)。
const STD_EXPR_MACROS: &[&str] = &[
    "println", "print", "eprintln", "eprint", "format", "format_args", "write", "writeln", "panic", "assert", "assert_eq",
    "assert_ne", "debug_assert", "debug_assert_eq", "debug_assert_ne", "vec", "dbg", "todo", "unimplemented", "unreachable",
    "ready", "matches",
];

/// 受け手の型推定で「境界」と数えない目印の trait (method を持たない / 持っていても呼ばれ方が違う)。
const MARKER_TRAITS: &[&str] = &["Send", "Sync", "Sized", "Unpin", "Copy", "UnwindSafe", "RefUnwindSafe"];

/// 境界の並び (`T: A + Send` / `dyn A + Send` / `impl A`) から、目印でない trait がちょうど 1 つならそれ。
/// 型引数や dyn の受け手の `x.f()` は、その trait で宣言された f にしか解決されない (inherent は無い)。
pub(crate) fn single_trait_bound<'b>(bounds: impl Iterator<Item = &'b syn::TypeParamBound>, imp: ImportLookup) -> Option<TyRef> {
    let traits: Vec<&syn::Path> = bounds
        .filter_map(|b| match b {
            syn::TypeParamBound::Trait(t) => Some(&t.path),
            _ => None,
        })
        .filter(|p| p.segments.last().is_some_and(|s| !MARKER_TRAITS.contains(&s.ident.to_string().as_str())))
        .collect();
    let [p] = traits.as_slice() else { return None };
    let last = p.segments.last()?.ident.to_string();
    Some(TyRef { name: unalias_of(p, last, imp), root: type_root_of(p, imp), ..Default::default() })
}

/// generics (型引数の境界 + where 句) から「型引数名 → 単一の trait 境界」を作る。
pub(crate) fn generic_bounds(g: &syn::Generics, imp: ImportLookup) -> HashMap<String, TyRef> {
    let mut all: HashMap<String, Vec<syn::TypeParamBound>> = HashMap::new();
    for t in g.type_params() {
        all.entry(t.ident.to_string()).or_default().extend(t.bounds.iter().cloned());
    }
    if let Some(w) = &g.where_clause {
        for p in &w.predicates {
            if let syn::WherePredicate::Type(pt) = p
                && let syn::Type::Path(tp) = &pt.bounded_ty
                && let Some(id) = tp.path.get_ident()
                && let Some(v) = all.get_mut(&id.to_string())
            {
                v.extend(pt.bounds.iter().cloned());
            }
        }
    }
    all.into_iter().filter_map(|(k, v)| single_trait_bound(v.iter(), imp).map(|t| (k, t))).collect()
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
    recv: Recv,                 // method 呼びの受け手の型の手掛かり (無ければ空)
    qual_root: String,          // 修飾の出どころ (`Error::new` の Error が `use std::io::Error` なら "std")
}

/// 1 関数ボディ内の呼び出し箇所を集める。
#[derive(Default)]
pub(crate) struct CallCollector {
    calls: Vec<RawCall>,
    /// このボディで束縛された名前 (let / 引数 / for / match / closure)。
    /// 値参照の候補から局所変数を落とすためのスコープ近似 — 無いと `len` / `path` / `id` のような
    /// ありふれた名前が method 名と衝突し、enchudb で候補が 14,738 行に膨れた。
    locals: std::collections::HashSet<String>,
    /// 束縛名 → 型の手掛かり (None = 束縛はあるが型は書いていない = シャドーイングで外の型を隠す)。
    /// block / closure ごとに積む。match / for / if let の束縛は囲む scope に「型不明」で漏れる (安全側)。
    scopes: Vec<HashMap<String, Option<Recv>>>,
    /// 型引数の名前 (fn と impl の generics)。`x: T` の T は具体型ではないので手掛かりにしない。
    generics: HashSet<String>,
    /// 型引数のうち trait 境界がちょうど 1 つの物 (`T: Store` の T → Store)。`x: T` の `x.f()` は Store::f。
    bounds: HashMap<String, TyRef>,
    /// 自作マクロ (repo の macro_rules 等) の引数の中か。トークンを組み替え得る (`delegate_call!(self.f())` の
    /// self は中身の値に書き換わる) ので、受け手に基づく確定をしない。std の式マクロは中身をそのまま使う。
    in_user_macro: bool,
    /// `self: Pin<&mut Self>` / `self: Box<Self>` のように受け手の型を明示した fn の中か。`self.f()` は
    /// 包んでいる型 (Pin 等) の method に先に当たる (`self.get_mut()` は Pin::get_mut)。
    self_wrapped: bool,
    /// 囲む impl の自分の型の出どころ (`impl Kill for StdChild` の StdChild が std なら "std")。
    /// 外の型への impl の中の `self.f()` は、std の inherent method が trait の method より優先される。
    self_root: String,
    /// この file の `use`: 名前 → (出どころ = 先頭 seg, 元の名前)。`use std::io::Error` なら
    /// Error → ("std", "Error")、`use crate::a::Db as MyDb` なら MyDb → ("crate", "Db")。
    imports: std::rc::Rc<HashMap<String, (String, String)>>,
    /// 関数の中の `use` (`use std::os::unix::fs::symlink;`)。file の `use` より優先。
    fn_imports: HashMap<String, (String, String)>,
}

/// 修飾なしの `f()` の f がこの body の束縛 (クロージャ / 引数) だった印。qual_root 列に入れて永続化する
/// (識別子になり得ない文字列 = どの crate 名とも一致しない → 解決時に「repo の関数ではない」)。
pub(crate) const LOCAL_BINDING_ROOT: &str = "(local)";

impl CallCollector {
    fn import(&self, name: &str) -> Option<&(String, String)> {
        self.fn_imports.get(name).or_else(|| self.imports.get(name))
    }
    fn is_bound(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains_key(name))
    }
    /// 関数呼び `a::b::f(..)` の (修飾ヒント = 末尾手前 seg (Type / module、単一 seg なら None), 出どころ)。
    /// 修飾なしの f がこの body の束縛なら出どころは LOCAL_BINDING_ROOT (`let f = |..| ..; f()` はクロージャ)。
    fn call_quals(&self, path: &syn::Path) -> (Option<String>, String) {
        let segs = &path.segments;
        let qualifier = (segs.len() >= 2).then(|| segs[segs.len() - 2].ident.to_string());
        let root = match segs.last() {
            Some(last) if segs.len() == 1 && self.is_bound(&last.ident.to_string()) => LOCAL_BINDING_ROOT.to_string(),
            _ => self.root_of(path),
        };
        (qualifier, root)
    }

    /// path (`T` / `a::T` / `a::T::f` の型部分まで) の出どころ。複数 seg は先頭 seg を `use` で引き直す
    /// (`use std::fs;` の後の `fs::DirEntry` は std)。単独の名前は `use` にあればその先頭、無ければ空 (ローカル)。
    /// 末尾 seg (関数名) は見ない: `path` が `T::f` なら T の出どころ。
    fn root_of(&self, path: &syn::Path) -> String {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if segs.len() <= 2 && path.leading_colon.is_none() {
            // `f` / `T::f` — 単独の名前 (T) の出どころは `use` だけが知っている
            return segs.first().and_then(|t| self.import(t)).map(|i| i.0.clone()).unwrap_or_default();
        }
        let first = segs[0].clone();
        match first.as_str() {
            "crate" | "self" | "super" | "Self" => first,
            _ => self.import(&first).map(|i| i.0.clone()).unwrap_or(first),
        }
    }
    /// この collector の `use` の引き方 (関数の中の `use` → file の `use`)。
    fn imp(&self) -> impl Fn(&str) -> Option<(String, String)> + '_ {
        move |n: &str| self.import(n).cloned()
    }
    fn lookup(&self, name: &str) -> Option<Recv> {
        self.scopes.iter().rev().find_map(|s| s.get(name)).cloned().flatten()
    }
    fn bind(&mut self, name: String, r: Option<Recv>) {
        if self.scopes.is_empty() {
            self.scopes.push(HashMap::new());
        }
        self.scopes.last_mut().unwrap().insert(name, r);
    }
    /// 書いてある型 → 手掛かり。`&T` / `&mut T` / `T<..>` は T。型引数・`impl`/`dyn`・tuple 等は無し。
    fn type_recv(&self, ty: &syn::Type) -> Option<Recv> {
        let imp = self.imp();
        let t = match ty {
            syn::Type::Reference(r) => return self.type_recv(&r.elem),
            syn::Type::Paren(p) => return self.type_recv(&p.elem),
            // `dyn Trait` / `impl Trait` — 境界の trait の method に解決される
            syn::Type::TraitObject(o) => single_trait_bound(o.bounds.iter(), &imp)?,
            syn::Type::ImplTrait(i) => single_trait_bound(i.bounds.iter(), &imp)?,
            // 型引数 `T` — 境界がちょうど 1 つなら、その trait
            syn::Type::Path(p) if p.qself.is_none() && p.path.segments.len() == 1 && self.generics.contains(&p.path.segments[0].ident.to_string()) => {
                self.bounds.get(&p.path.segments[0].ident.to_string())?.clone()
            }
            _ => ty_ref(ty, &imp, &self.generics)?,
        };
        Some(Recv { ty: t.name, root: t.root, ..Default::default() })
    }
    /// 式の型の手掛かり: `T { .. }` / `T::f(..)` / `x` / `self` を起点に、`.field` / `.m(..)` / `?` を
    /// 手順として積む (`let y = x.a()?;` の y、`self.cfg.run()` の受け手)。型そのものは解決時にたどる。
    fn expr_recv(&self, e: &syn::Expr) -> Option<Recv> {
        fn step(mut r: Recv, s: String) -> Option<Recv> {
            if r.chain.split(' ').filter(|x| !x.is_empty()).count() >= CHAIN_MAX {
                return None;
            }
            if !r.chain.is_empty() {
                r.chain.push(' ');
            }
            r.chain.push_str(&s);
            Some(r)
        }
        match e {
            syn::Expr::Struct(s) if s.qself.is_none() => {
                let last = s.path.segments.last()?.ident.to_string();
                let imp = self.imp();
                Some(Recv { ty: unalias_of(&s.path, last, &imp), root: type_root_of(&s.path, &imp), ..Default::default() })
            }
            syn::Expr::Call(c) => {
                let syn::Expr::Path(p) = &*c.func else { return None };
                if p.qself.is_some() {
                    return None;
                }
                let segs = &p.path.segments;
                let f = segs.last()?.ident.to_string();
                let (qualifier, root) = self.call_quals(&p.path);
                match qualifier {
                    // `Box::pin(x)` / `Arc::new(x)` — 包んだ中身の method に自動参照外しで届く。包んだ印を手順に積む
                    Some(q) if SMART_PTRS.contains(&q.as_str()) && SMART_PTR_CTORS.contains(&f.as_str()) && c.args.len() == 1 => {
                        step(self.expr_recv(&c.args[0])?, format!("w:{q}"))
                    }
                    // `T::f(..)` — 型っぽい修飾 (大文字始まり / Self)。型引数 `T::new()` は中身が分からない
                    Some(q) if q.starts_with(|c: char| c.is_ascii_uppercase()) => {
                        (!(segs.len() == 2 && self.generics.contains(&q))).then(|| Recv { ty: q, via_fn: f, root, ..Default::default() })
                    }
                    // 自由関数 `f(..)` / `module::f(..)` — 戻り値の型は解決時に呼び先を 1 つに絞れた時だけ使う
                    q => Some(Recv { ty: format!("{FREE_FN_RECV}{}", q.unwrap_or_default()), via_fn: f, root, ..Default::default() }),
                }
            }
            syn::Expr::Try(t) => {
                let r = self.expr_recv(&t.expr)?;
                if r.chain.is_empty() && !r.via_fn.is_empty() && !r.via_try {
                    Some(Recv { via_try: true, ..r })
                } else {
                    step(r, "?".into())
                }
            }
            syn::Expr::Field(f) => match &f.member {
                syn::Member::Named(id) => step(self.expr_recv(&f.base)?, format!("f:{id}")),
                syn::Member::Unnamed(_) => None,
            },
            syn::Expr::MethodCall(m) => {
                if self.self_wrapped && matches!(&*m.receiver, syn::Expr::Path(p) if p.path.is_ident("self")) {
                    return None; // `self: Pin<&mut Self>` の `self.m()` は Pin の method に先に当たり得る
                }
                step(self.expr_recv(&m.receiver)?, format!("m:{}", m.method))
            }
            syn::Expr::Reference(r) => self.expr_recv(&r.expr),
            // `*x` — 変数 / field の参照外しは method 呼びの自動参照外しと同じ先に届く。ただし `*self` は別:
            // `impl Matcher for &M` / `impl Sink for Box<S>` の中では Self 自体が参照 / ポインタで、外すと中身
            // (M / S) に変わる (`(*self).find_at()` を自分自身に確定した実例)。起点が素の self なら推定しない。
            syn::Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => {
                self.expr_recv(&u.expr).filter(|r| !(r.ty == "Self" && r.chain.is_empty() && r.via_fn.is_empty()))
            }
            syn::Expr::Paren(p) => self.expr_recv(&p.expr),
            syn::Expr::Group(g) => self.expr_recv(&g.expr),
            // `self.field` は Pin<&mut Self> 越しでも Self の field に届く (自動参照外し)。当たり得ないのは
            // `self.m()` の直接の method 呼びだけ (下の MethodCall で止める)。
            syn::Expr::Path(p) if p.path.is_ident("self") => Some(Recv { ty: "Self".into(), root: self.self_root.clone(), ..Default::default() }),
            syn::Expr::Path(p) => p.path.get_ident().and_then(|i| self.lookup(&i.to_string())),
            _ => None,
        }
    }
    /// fn の仮引数を束縛として登録 (型が書いてあれば手掛かり付き)。
    fn bind_params(&mut self, sig: &syn::Signature) {
        for a in &sig.inputs {
            if let syn::FnArg::Typed(t) = a {
                self.visit_pat(&t.pat);
                if let syn::Pat::Ident(pi) = &*t.pat
                    && pi.subpat.is_none()
                {
                    let r = self.type_recv(&t.ty);
                    self.bind(pi.ident.to_string(), r);
                }
            }
        }
    }
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
            recv: Recv::default(),
            qual_root: String::new(),
        });
    }
}

impl<'ast> Visit<'ast> for CallCollector {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = &*node.func {
            let segs = &p.path.segments;
            if let Some(last) = segs.last() {
                let (qualifier, qual_root) = self.call_quals(&p.path);
                self.calls.push(RawCall {
                    name: last.ident.to_string(),
                    qualifier,
                    is_method: false,
                    self_recv: false,
                    line: line_of(last.ident.span()),
                    col: col_of(last.ident.span()),
                    as_value: false,
                    in_macro: false,
                    recv: Recv::default(),
                    qual_root,
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
        let self_recv = !self.in_user_macro && !self.self_wrapped && matches!(&*node.receiver, syn::Expr::Path(p) if p.path.is_ident("self"));
        // それ以外は、受け手が束縛名 `x` で、その型がソースに書いてある時だけ手掛かりを持つ。
        let bare_self = matches!(&*node.receiver, syn::Expr::Path(p) if p.path.is_ident("self"));
        let recv = if self.in_user_macro || (bare_self && self.self_wrapped) {
            Recv::default() // `self: Pin<&mut Self>` の `self.f()` は Pin の method に先に当たり得る
        } else if self_recv {
            Recv { root: self.self_root.clone(), ..Default::default() }
        } else {
            self.expr_recv(&node.receiver).unwrap_or_default()
        };
        self.calls.push(RawCall {
            name: node.method.to_string(),
            qualifier: None,
            is_method: true,
            self_recv,
            line: line_of(node.method.span()),
            col: col_of(node.method.span()),
            as_value: false,
            in_macro: false,
            recv,
            qual_root: String::new(),
        });
        visit::visit_expr_method_call(self, node);
    }
    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        self.path_as_value(node);
        visit::visit_expr_path(self, node);
    }
    fn visit_pat_ident(&mut self, node: &'ast syn::PatIdent) {
        self.locals.insert(node.ident.to_string()); // let / 引数 / for / match / closure の束縛
        self.bind(node.ident.to_string(), None); // 型は不明として外の同名を隠す (型付きは呼び側で上書き)
        visit::visit_pat_ident(self, node);
    }
    /// 関数の中の `use`。ブロック境界は見ない (関数全体に効かせる = 近似、出どころの判定にだけ使う)。
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        use_tree_imports(&node.tree, None, &mut self.fn_imports);
    }
    fn visit_block(&mut self, node: &'ast syn::Block) {
        self.scopes.push(HashMap::new());
        // ブロックの中の `fn` は宣言より前からも見える。index していない (関数の中の関数) ので、同名の外の fn に
        // 確定しないよう、先にローカルの束縛として登録する (tokio の tests の中の `fn iter()` を tokio_stream::iter
        // に確定していた)。
        for st in &node.stmts {
            if let syn::Stmt::Item(syn::Item::Fn(f)) = st {
                self.locals.insert(f.sig.ident.to_string());
                self.bind(f.sig.ident.to_string(), None);
            }
        }
        visit::visit_block(self, node);
        self.scopes.pop();
    }
    /// クロージャの引数は型が書いてあれば手掛かりにする (`|dir: Dir, mut cmd: TestCommand| cmd.arg(..)`)。
    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        self.scopes.push(HashMap::new());
        for p in &node.inputs {
            self.visit_pat(p);
            if let syn::Pat::Type(pt) = p
                && let syn::Pat::Ident(pi) = &*pt.pat
                && pi.subpat.is_none()
            {
                let r = self.type_recv(&pt.ty);
                self.bind(pi.ident.to_string(), r);
            }
        }
        self.visit_expr(&node.body);
        self.scopes.pop();
    }
    /// `let` は **init を先に**歩く (`let x = x.f()` の右辺の x は外の x)。その後で束縛する。
    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let Some(init) = &node.init {
            self.visit_expr(&init.expr);
            if let Some((_, diverge)) = &init.diverge {
                self.visit_expr(diverge);
            }
        }
        self.visit_pat(&node.pat);
        let (ident, r) = match &node.pat {
            syn::Pat::Type(pt) => match &*pt.pat {
                syn::Pat::Ident(pi) if pi.subpat.is_none() => (Some(pi.ident.to_string()), self.type_recv(&pt.ty)),
                _ => (None, None),
            },
            syn::Pat::Ident(pi) if pi.subpat.is_none() => {
                // `let x = ... else { .. }` (let-else) は refutable パターンなので init の型と x の型が違う
                let r = node.init.as_ref().filter(|i| i.diverge.is_none()).and_then(|i| self.expr_recv(&i.expr));
                (Some(pi.ident.to_string()), r)
            }
            _ => (None, None),
        };
        if let Some(i) = ident {
            self.bind(i, r);
        }
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
            // parse した式は所有物 ('ast ではない) なので、別 collector で回して結果だけ貰う。束縛・use・型引数は
            // 引き継ぐ (`println!("{}", x.f())` の x の型)。std 以外のマクロの中では受け手に基づく確定をしない。
            let name = node.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
            let mut sub = CallCollector {
                scopes: self.scopes.clone(),
                generics: self.generics.clone(),
                bounds: self.bounds.clone(),
                imports: self.imports.clone(),
                fn_imports: self.fn_imports.clone(),
                self_root: self.self_root.clone(),
                self_wrapped: self.self_wrapped,
                in_user_macro: self.in_user_macro || !STD_EXPR_MACROS.contains(&name.as_str()),
                ..Default::default()
            };
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
#[derive(Clone)]
pub(crate) struct SymDef {
    eid: EntityId,
    kind: u32,         // method 呼び (x.foo()) は method 定義のみを対象にするため
    container: String, // impl 型 (method) / "" (free fn)。`Type::` 修飾の突き合わせ用
    module: String,    // "a::b"。`mod::` 修飾の突き合わせ用
    crate_: String,    // 定義元 crate。`mycrate::fn` / `enchudb_schema::foo` の突き合わせ用
    ret: TyRef,        // 戻り値の型。`T::new()` / `x.a()` の型を決める (出どころ付き)
    impl_trait: TyRef, // trait 実装の method ならその trait (name / root)
    impl_blanket: bool, // 型引数を含む型への trait 実装
    file: EntityId,     // 定義のある file (別 crate / 別ターゲットの判定)
    gated: bool,        // `cfg_not_*!` の中の定義 (確定先にしない)
    cond: bool,         // cfg 付きの定義 (別の file からは確定しない)
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
    pub(crate) recv: Recv,     // method 呼びの受け手の型の手掛かり
    pub(crate) qual_root: String, // 修飾の出どころ (repo の外なら修飾一致で確定しない)
    pub(crate) line: u32, // callee ident 行 (1-indexed)
    pub(crate) col: u32,  // callee ident 列 (0-indexed)
}

/// 1 つの `impl Trait for Type`。go-to-implementation 用の edge。
pub(crate) struct ImplEdge {
    pub(crate) trait_name: String,
    pub(crate) type_name: String,
    pub(crate) for_all: bool, // `impl<R: Buf> BufExt for R` — 自分の型が型引数そのもの (拡張 trait の形)
    pub(crate) file: EntityId,
    pub(crate) line: u32,
}

/// pass1/pass2 をまたいで貯める累積器 (ファイル横断)。
#[derive(Default)]
pub(crate) struct Acc {
    pub(crate) defs: HashMap<String, Vec<SymDef>>, // name → 定義群 (syn 解決用)
    pub(crate) pending: Vec<CallSite>,             // 未解決 call-site
    pub(crate) impls: Vec<ImplEdge>,               // impl Trait for Type edge
    pub(crate) fields: Vec<(EntityId, RawField)>,  // struct の field の型 (file eid 付き、挿入は呼び側)
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
    impl_trait: TyRef,     // 囲む `impl Trait for T` の Trait (名前 + 出どころ)。inherent impl なら空
    impl_blanket: bool,    // その impl の自分の型が型引数そのもの / それを包む参照・ポインタ (`impl<P> Tr for Pin<P>` / `for &M`)
    impl_self_root: String, // その impl の自分の型の出どころ (外の型への impl を見分ける)
    neg_gate: bool,        // `cfg_not_*! { .. }` の中 = 「その機能が無い時」の代用品の定義
    cond: bool,            // cfg 付きの inline module / impl の中 (build の設定で有無が変わる定義)
    impl_generics: Vec<String>, // 囲む impl の型引数 (受け手の型推定で具体型と区別する)
    impl_bounds: HashMap<String, TyRef>, // 囲む impl の型引数の単一 trait 境界
    imports: std::rc::Rc<HashMap<String, (String, String)>>, // この file の `use` (名前 → (出どころ, 元の名前))
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
    pub(crate) fields: Vec<RawField>,
    /// `cfg_not_*!` の中で宣言された子 module 名 (`cfg_not_has_atomic_u64! { mod atomic_u64_as_mutex; }`)。
    pub(crate) gated_mods: Vec<String>,
    /// cfg_not の外で宣言された子 module 名。同じ名前が両方の枝にあれば (`cfg_rt! { mod runtime; }` と
    /// `cfg_not_rt! { mod runtime; }`) 代用品ではなく同じ module の出し分けなので除外しない。
    live_mods: Vec<String>,
    /// `#[cfg(..)] mod x;` (cfg(test) を除く) — 条件付きの module。外から (cfg 付きの re-export 越しに) 呼ぶと
    /// build の設定次第で別の実体に切り替わり得る (tokio の parking_lot::Condvar ↔ std の Condvar)。
    pub(crate) cond_mods: Vec<String>,
    /// この file で宣言した子 module の名前 (`#[path]` で file 名と違う名前でも repo の内側の名前)。
    pub(crate) mod_names: Vec<String>,
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
    pub(crate) ret: TyRef,    // fn / method の戻り値の型 (連鎖 `x.a().b()` をたどる材料。無ければ空)
    pub(crate) impl_trait: TyRef, // trait 実装の method ならその trait (確定先を RA の流儀に揃える)
    pub(crate) impl_blanket: bool, // 型引数を含む型への trait 実装 (method 呼びでも RA は trait の宣言を指す)
    pub(crate) gated: bool,        // `cfg_not_*!` の中の定義 (代用品。確定先にしない)
    pub(crate) cond: bool,         // cfg 付き (build の設定で有無が変わる)。別の file からは確定しない
    pub(crate) calls: Vec<RawCall>,
}

/// struct の名前付き field 1 つとその型 (連鎖 `self.cfg.run()` をたどる材料)。
pub(crate) struct RawField {
    pub(crate) strukt: String,
    pub(crate) name: String,
    pub(crate) ty: TyRef,
}

/// `impl Trait for Type` (file eid は挿入時に付く)。
pub(crate) struct RawImpl {
    trait_name: String,
    type_name: String,
    for_all: bool,
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
        let mut cc = CallCollector { imports: ctx.imports.clone(), self_root: ctx.impl_self_root.clone(), ..Default::default() };
        cc.generics.extend(ctx.impl_generics.iter().cloned());
        cc.bounds.extend(ctx.impl_bounds.iter().map(|(k, v)| (k.clone(), v.clone())));
        // 引数名は signature 側にあるので、body だけ歩くと束縛が漏れる。
        // (tokio の `registration` のような仮引数が「同名関数への参照」に化けていた)
        if let Some(s) = sig {
            cc.self_wrapped = matches!(s.inputs.first(), Some(syn::FnArg::Receiver(r)) if r.colon_token.is_some());
            cc.generics.extend(s.generics.type_params().map(|t| t.ident.to_string()));
            let fb = {
                let imp = |n: &str| ctx.imports.get(n).cloned();
                generic_bounds(&s.generics, &imp)
            };
            cc.bounds.extend(fb); // fn の境界が impl の同名より内側 (上書き)
            cc.bind_params(s);
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
        ret: sig.and_then(|s| match &s.output {
            syn::ReturnType::Type(_, ty) => {
                let generics: HashSet<String> = ctx.impl_generics.iter().cloned().chain(s.generics.type_params().map(|t| t.ident.to_string())).collect();
                let imp = |n: &str| ctx.imports.get(n).cloned();
                ty_ref(ty, &imp, &generics)
            }
            syn::ReturnType::Default => None,
        })
        .unwrap_or_default(),
        impl_trait: if kind == K_METHOD { ctx.impl_trait.clone() } else { TyRef::default() },
        impl_blanket: kind == K_METHOD && ctx.impl_blanket,
        gated: ctx.neg_gate,
        cond: ctx.cond || attrs.iter().any(is_cfg_cond),
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
        .set("gated_mods", facts.gated_mods.join(" ").as_str())
        .set("cond_mods", facts.cond_mods.join(" ").as_str())
        .set("mod_names", facts.mod_names.join(" ").as_str())
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
            .set("ret_ty", s.ret.name.as_str())
            .set("ret_root", s.ret.root.as_str())
            .set("ret_arg", s.ret.arg.as_str())
            .set("ret_arg_root", s.ret.arg_root.as_str())
            .set("impl_trait", s.impl_trait.name.as_str())
            .set("impl_trait_root", s.impl_trait.root.as_str())
            .set("impl_blanket", s.impl_blanket as u32)
            .set("gated", s.gated as u32)
            .set("cond", s.cond as u32)
            .set("doc", s.doc.as_str())
        .set("attrs", s.attrs.as_str())
            .set("line", s.line)
            .set("end_line", s.end_line)
            .commit()
            .unwrap();
        // 名前解決の突き合わせ先として登録 (syn 用)。
        acc.defs.entry(s.name).or_default().push(SymDef { eid: sym_eid, kind: s.kind, container: s.container.clone(), module: s.module, crate_: crate_name.clone(), ret: s.ret.clone(), impl_trait: s.impl_trait.clone(), impl_blanket: s.impl_blanket, file: file_eid, gated: s.gated, cond: s.cond });
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
                recv: rc.recv,
                qual_root: rc.qual_root,
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
            recv: rc.recv,
            qual_root: rc.qual_root,
            line: rc.line,
            col: rc.col,
        });
    }
    for im in facts.impls {
        acc.impls.push(ImplEdge { trait_name: im.trait_name, type_name: im.type_name, for_all: im.for_all, file: file_eid, line: im.line });
    }
    acc.fields.extend(facts.fields.into_iter().map(|f| (file_eid, f)));
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
                            recv: Recv::default(),
                            qual_root: String::new(),
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

/// impl の自分の型が「型引数そのもの / それを包む参照・ポインタ」か (`impl<T> Tr for T` / `for &M` /
/// `for Box<T>` / `for Pin<P>`)。この形の impl の method を呼ぶと rust-analyzer は trait の宣言を指す。
/// `impl<R> Tr for BufReader<R>` は具体的な型の impl (RA は impl の method を指す)。
pub(crate) fn is_blanket_self(ty: &syn::Type, params: &HashSet<String>) -> bool {
    match ty {
        syn::Type::Reference(r) => is_blanket_self(&r.elem, params),
        syn::Type::Paren(p) => is_blanket_self(&p.elem, params),
        syn::Type::Path(p) if p.qself.is_none() => {
            let Some(last) = p.path.segments.last() else { return false };
            if p.path.segments.len() == 1 && params.contains(&last.ident.to_string()) {
                return true;
            }
            if !matches!(last.ident.to_string().as_str(), "Box" | "Pin" | "Rc" | "Arc") {
                return false;
            }
            match &last.arguments {
                syn::PathArguments::AngleBracketed(a) => a.args.iter().any(|g| matches!(g, syn::GenericArgument::Type(t) if is_blanket_self(t, params))),
                _ => false,
            }
        }
        _ => false,
    }
}

/// file module 宣言の記録形: `name`、`#[path = "p"]` 付きなら `name@p` (両方の枝で同じ名前でも別 file を指す:
/// tokio の `#[path = "atomic_u64_as_mutex.rs"] mod imp;` と `#[path = "atomic_u64_native.rs"] mod imp;`)。
fn mod_decl_entry(m: &syn::ItemMod) -> String {
    let path = m.attrs.iter().find(|a| a.path().is_ident("path")).and_then(|a| match &a.meta {
        syn::Meta::NameValue(nv) => match &nv.value {
            syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => Some(s.value()),
            _ => None,
        },
        _ => None,
    });
    match path {
        Some(p) => format!("{}@{p}", m.ident),
        None => m.ident.to_string(),
    }
}

/// `#[cfg(..)]` のうち cfg(test) 以外 (= build の設定で有無が変わる)。
pub(crate) fn is_cfg_cond(a: &syn::Attribute) -> bool {
    a.path().is_ident("cfg") && !is_cfg_test(a)
}

/// module 宣言 (`mod_decl_entry` の形) → (その file, その下の dir)。`decl` = 宣言している file。
/// 親が mod.rs / lib.rs / main.rs なら同じ dir、それ以外は親の名前の dir の下。`#[path]` は宣言している file の dir から。
pub(crate) fn mod_targets(decl: &Path, entry: &str) -> Option<(PathBuf, PathBuf)> {
    let dir = decl.parent()?;
    if let Some((_, p)) = entry.split_once('@') {
        let f = dir.join(p);
        return Some((f.clone(), f.with_extension("")));
    }
    let stem = decl.file_stem()?.to_str()?;
    let base = if matches!(stem, "mod" | "lib" | "main") { dir.to_path_buf() } else { dir.join(stem) };
    Some((base.join(format!("{entry}.rs")), base.join(entry)))
}

/// マクロの本体を item の並びとして読む (`cfg_rt! { .. }` の中身)。1 つでも読めなければ Err。
fn parse_items<T: syn::parse::Parse>(input: syn::parse::ParseStream) -> syn::Result<Vec<T>> {
    let mut v = Vec::new();
    while !input.is_empty() {
        v.push(input.parse()?);
    }
    Ok(v)
}

/// impl の中身。`cfg_rt! { fn f() {} }` のように impl の中で method を包むマクロも読む。
fn walk_impl_items(items: &[syn::ImplItem], ctx: &mut Ctx, out: &mut FileFacts) {
    for ii in items {
        match ii {
            syn::ImplItem::Fn(m) => {
                let is_test = ctx.in_test || is_test_attrs(&m.attrs);
                record_symbol(
                    ctx, out, &m.sig.ident.to_string(), K_METHOD,
                    classify_vis(&m.vis), m.sig.asyncness.is_some(), is_test,
                    line_of(m.sig.ident.span()), col_of(m.sig.ident.span()), end_line_of(m), Some(&m.sig), Some(&m.block),
                    &m.attrs,
                );
            }
            syn::ImplItem::Macro(m) => {
                if let Ok(inner) = m.mac.parse_body_with(parse_items::<syn::ImplItem>) {
                    walk_impl_items(&inner, ctx, out);
                }
            }
            _ => {}
        }
    }
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
        syn::Item::Struct(s) => {
            record_symbol(
                ctx, out, &s.ident.to_string(), K_STRUCT,
                classify_vis(&s.vis), false, ctx.in_test, line_of(s.ident.span()), col_of(s.ident.span()), end_line_of(s), None, None,
                &s.attrs,
            );
            // 名前付き field の型 (struct の型引数は具体型ではないので除く)。
            let generics: HashSet<String> = s.generics.type_params().map(|t| t.ident.to_string()).collect();
            let imp = |n: &str| ctx.imports.get(n).cloned();
            if let syn::Fields::Named(named) = &s.fields {
                for f in &named.named {
                    if let (Some(id), Some(t)) = (&f.ident, ty_ref(&f.ty, &imp, &generics)) {
                        out.fields.push(RawField { strukt: s.ident.to_string(), name: id.to_string(), ty: t });
                    }
                }
            }
        }
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
                    let params: HashSet<String> = i.generics.type_params().map(|t| t.ident.to_string()).collect();
                    out.impls.push(RawImpl {
                        trait_name: seg.ident.to_string(),
                        type_name: type_name.clone(),
                        for_all: matches!(&*i.self_ty, syn::Type::Path(p) if p.qself.is_none() && p.path.get_ident().is_some_and(|id| params.contains(&id.to_string()))),
                        line: line_of(seg.ident.span()),
                    });
                }
            let prev = std::mem::replace(&mut ctx.container, type_name);
            // trait 実装の method は呼び出し側に名前が出ない (trait 経由 / dyn) ため、
            // 「誰からも呼ばれていない」に構造的に見える。区別できるよう印を付ける。
            let prev_ti = std::mem::replace(&mut ctx.in_trait_impl, i.trait_.is_some());
            let prev_g = std::mem::replace(&mut ctx.impl_generics, i.generics.type_params().map(|t| t.ident.to_string()).collect());
            let bounds = {
                let imp = |n: &str| ctx.imports.get(n).cloned();
                generic_bounds(&i.generics, &imp)
            };
            let prev_b = std::mem::replace(&mut ctx.impl_bounds, bounds);
            let tr = i.trait_.as_ref().and_then(|(_, p, _)| {
                let last = p.segments.last()?.ident.to_string();
                let imp = |n: &str| ctx.imports.get(n).cloned();
                Some(TyRef { name: unalias_of(p, last, &imp), root: type_root_of(p, &imp), ..Default::default() })
            });
            let prev_tr = std::mem::replace(&mut ctx.impl_trait, tr.unwrap_or_default());
            let params: HashSet<String> = i.generics.type_params().map(|t| t.ident.to_string()).collect();
            let blanket = is_blanket_self(&i.self_ty, &params);
            let prev_bl = std::mem::replace(&mut ctx.impl_blanket, blanket);
            let self_root = match &*i.self_ty {
                syn::Type::Path(p) if p.qself.is_none() => {
                    let imp = |n: &str| ctx.imports.get(n).cloned();
                    type_root_of(&p.path, &imp)
                }
                _ => String::new(),
            };
            let prev_sr = std::mem::replace(&mut ctx.impl_self_root, self_root);
            let c = ctx.cond || i.attrs.iter().any(is_cfg_cond);
            let prev_c = std::mem::replace(&mut ctx.cond, c);
            walk_impl_items(&i.items, ctx, out);
            ctx.cond = prev_c;
            ctx.impl_self_root = prev_sr;
            ctx.impl_blanket = prev_bl;
            ctx.impl_trait = prev_tr;
            ctx.container = prev;
            ctx.in_trait_impl = prev_ti;
            ctx.impl_generics = prev_g;
            ctx.impl_bounds = prev_b;
        }
        // item 直下のマクロ: `criterion_group!(benches, bench_tie, …)` のように **関数の外**で
        // 定義を参照する形。CallCollector は関数本体しか歩かないので、ここを見ないと
        // bench 関数群が丸ごと「誰からも呼ばれていない」に見える (実際そう見えていた)。
        syn::Item::Macro(m) => {
            use syn::punctuated::Punctuated;
            if m.mac.path.is_ident("macro_rules") {
                return; // マクロの定義そのもの (中身はパターン)
            }
            // `cfg_rt! { pub fn f() {} impl X {..} }` のように item を包むマクロ (tokio は定義の多くがこれ)。
            // 中身を読まないと定義が見えず、同名の「見えている方」を一意と思い込んで誤確定する。
            if let Ok(items) = m.mac.parse_body_with(parse_items::<syn::Item>)
                && !items.is_empty()
            {
                // `cfg_not_*! { .. }` は「その機能が無い時」の代用品 (tokio の loom の AtomicU64 等)
                let neg = m.mac.path.segments.last().is_some_and(|s| s.ident.to_string().starts_with("cfg_not"));
                let gate = ctx.neg_gate || neg;
                let prev = std::mem::replace(&mut ctx.neg_gate, gate);
                walk_items(&items, ctx, out);
                ctx.neg_gate = prev;
                return;
            }
            if let Ok(args) = m.mac.parse_body_with(Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated) {
                let mut cc = CallCollector { imports: ctx.imports.clone(), ..Default::default() };
                for e in &args {
                    visit::visit_expr(&mut cc, e);
                }
                out.item_calls.append(&mut cc.finish());
            } else {
                scan_tokens_for_calls(m.mac.tokens.clone(), &mut out.item_calls);
            }
        }
        syn::Item::Mod(m) => {
            let entry = mod_decl_entry(m);
            out.mod_names.push(m.ident.to_string());
            // 属性の `cfg(not(..))` は代用品とは限らない (tokio の `#[cfg(not(loom))] mod std;` は普段の build の本体)。
            // 意図が名前に出ている `cfg_not_*!` の中の宣言だけを代用品とみなす。
            let neg = ctx.neg_gate;
            let cond = m.attrs.iter().any(is_cfg_cond);
            if let Some((_, inner)) = &m.content {
                let test = ctx.in_test || m.attrs.iter().any(is_cfg_test);
                ctx.module.push(m.ident.to_string());
                let prev = std::mem::replace(&mut ctx.in_test, test);
                let prev_n = std::mem::replace(&mut ctx.neg_gate, neg);
                let c = ctx.cond || cond;
                let prev_c = std::mem::replace(&mut ctx.cond, c);
                walk_items(inner, ctx, out);
                ctx.cond = prev_c;
                ctx.neg_gate = prev_n;
                ctx.in_test = prev;
                ctx.module.pop();
            } else {
                if neg {
                    out.gated_mods.push(entry.clone()); // 子 file は Resolver が path から引く
                } else {
                    out.live_mods.push(entry.clone());
                }
                if cond || ctx.cond {
                    out.cond_mods.push(entry);
                }
            }
        }
        _ => {}
    }
}

// ─────────────────────────── 名前解決 (pass2) ───────────────────────────

/// 名前解決の材料: 定義表 + 「repo の内側」とみなす出どころ (crate 名 / module 名 / crate・self・super)。
/// 出どころが外 (`std` / 依存 crate) の型名・修飾は、同名の自前の定義と結び付けない。
pub(crate) struct Resolver<'a> {
    pub(crate) defs: &'a HashMap<String, Vec<SymDef>>,
    local_roots: HashSet<String>,
    fields: FieldTypes,
    /// file eid → (crate 名, path)。修飾なしの呼び出しが別 crate / 別ターゲットを指していないかを見る。
    files: HashMap<EntityId, (String, String)>,
    /// repo で宣言された trait の名前。
    traits: HashSet<String>,
    /// 「全ての型 (境界付き) に向けた impl」を持つ trait = 拡張 trait (`impl<R: Buf> BufExt for R {}`)。
    /// trait 境界の受け手でも、これらが同名 method を宣言していればどちらが呼ばれるか決められない。
    for_all_traits: HashSet<String>,
    /// repo で定義した型 / trait の名前。ここに無い型 (re-export された std の Arc 等) への trait 実装の method は、
    /// その型の inherent method (`Arc::from_raw`) に隠され得るので確定先にしない。
    types: HashSet<String>,
    /// `cfg_not_*!` の中で宣言された module の file (= 代用品。普段の build では使われない)。
    gated_files: HashSet<EntityId>,
    /// cfg 付きで宣言された module の file → その範囲の番号。範囲の外からは確定しない。
    cond_region: HashMap<EntityId, usize>,
}

/// `use` せずに使える prelude の型。出どころが空 (= use していない) のこれらは std の型 (repo に同名の
/// `impl .. for Vec<T>` があっても、`v.extend()` は std の Vec::extend)。
const PRELUDE_TYPES: &[&str] = &["Vec", "String", "Box", "Option", "Result"];

/// tests/ benches/ examples/ の直下の file は、それぞれが別の crate (cargo のターゲット)。
fn target_root_file(path: &str) -> bool {
    Path::new(path).parent().and_then(|d| d.file_name()).and_then(|d| d.to_str()).is_some_and(|d| matches!(d, "tests" | "benches" | "examples"))
}
impl<'a> Resolver<'a> {
    /// `rs_paths` = index 対象の .rs (module 名 = file 名 / mod.rs の dir 名を内側に数える)。
    /// `file_t` = index 済みの file 表 (crate 名 / path / 言語)。
    pub(crate) fn new(defs: &'a HashMap<String, Vec<SymDef>>, file_t: &Table, fields: FieldTypes, for_all_traits: HashSet<String>) -> Self {
        let mut files: HashMap<EntityId, (String, String)> = HashMap::new();
        for e in file_t.where_eq("lang", LANG_RUST).find().unwrap_or_default() {
            let er = file_t.entity(e);
            files.insert(e, (crate_key(&txt(er.get("crate_"))), txt(er.get("path"))));
        }
        let traits: HashSet<String> = defs.iter().filter(|(_, v)| v.iter().any(|d| d.kind == K_TRAIT)).map(|(k, _)| k.clone()).collect();
        let types: HashSet<String> =
            defs.iter().filter(|(_, v)| v.iter().any(|d| matches!(d.kind, K_STRUCT | K_ENUM | K_TRAIT))).map(|(k, _)| k.clone()).collect();
        // module 宣言 → 対象の file / dir (mod_targets)。`cfg_not_*!` の中の宣言 = 代用品、cfg 付きの宣言 = 条件付き。
        let mut gated_targets: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut cond_targets: Vec<(PathBuf, PathBuf)> = Vec::new();
        for e in file_t.where_eq("lang", LANG_RUST).find().unwrap_or_default() {
            let er = file_t.entity(e);
            let p = PathBuf::from(txt(er.get("path")));
            for (col, out) in [("gated_mods", &mut gated_targets), ("cond_mods", &mut cond_targets)] {
                for m in txt(er.get(col)).split(' ').filter(|m| !m.is_empty()) {
                    out.extend(mod_targets(&p, m));
                }
            }
        }
        let region_of = |p: &str, ts: &[(PathBuf, PathBuf)]| {
            let p = Path::new(p);
            ts.iter().position(|(f, d)| p == f || p.starts_with(d))
        };
        let gated_files: HashSet<EntityId> = files.iter().filter(|(_, (_, p))| region_of(p, &gated_targets).is_some()).map(|(e, _)| *e).collect();
        let cond_region: HashMap<EntityId, usize> = files.iter().filter_map(|(e, (_, p))| region_of(p, &cond_targets).map(|r| (*e, r))).collect();
        let rs_paths: Vec<String> = files.values().map(|(_, p)| p.clone()).collect();
        let mut local_roots: HashSet<String> = ["crate", "self", "super", "Self"].iter().map(|s| s.to_string()).collect();
        for d in defs.values().flatten() {
            local_roots.insert(crate_key(&d.crate_));
            local_roots.extend(d.module.split("::").filter(|m| !m.is_empty()).map(str::to_string));
        }
        for e in file_t.where_eq("lang", LANG_RUST).find().unwrap_or_default() {
            local_roots.extend(txt(file_t.entity(e).get("mod_names")).split(' ').filter(|m| !m.is_empty()).map(str::to_string));
        }
        for p in &rs_paths {
            let p = Path::new(p);
            match p.file_stem().and_then(|s| s.to_str()) {
                Some("mod") => {
                    if let Some(d) = p.parent().and_then(|d| d.file_name()).and_then(|d| d.to_str()) {
                        local_roots.insert(d.to_string());
                    }
                }
                Some(s) => {
                    local_roots.insert(s.to_string());
                }
                None => {}
            }
        }
        // std / core / alloc は同名の module があっても常に外 (`use std::time::Instant` の std は標準ライブラリ。
        // tokio の loom/std/ module を内側と数えて Instant::now を自前に確定した実例)。
        for ext in ["std", "core", "alloc"] {
            local_roots.remove(ext);
        }
        Resolver { defs, local_roots, fields, files, traits, for_all_traits, types, gated_files, cond_region }
    }
    /// 呼び出し元 (file) から定義 `d` を確定先にしてよいか。cfg 付きの定義 / 条件付き module の中の定義は、
    /// 同じ file / 同じ条件付き module の中からの呼び出しだけ (外からは cfg 付きの re-export 越しに別の実体に
    /// 切り替わり得る)。代用品 (`cfg_not_*!`) はどこからも確定しない。
    fn live_for(&self, caller_file: EntityId, d: &SymDef) -> bool {
        if d.gated || self.gated_files.contains(&d.file) {
            return false;
        }
        if d.cond && caller_file != d.file {
            return false;
        }
        match self.cond_region.get(&d.file) {
            Some(r) => self.cond_region.get(&caller_file) == Some(r),
            None => true,
        }
    }
    /// 定義の module の道順: file の置き場所 (`src/time/sleep.rs` → time, sleep。lib.rs / main.rs / mod.rs は
    /// dir まで) + file 内の `mod x { }`。`#[path]` で置き場所を変えた file はずれる (その時は一致しないだけ)。
    fn module_of(&self, d: &SymDef) -> Vec<String> {
        let path = format!("/{}", self.files.get(&d.file).map(|f| f.1.as_str()).unwrap_or(""));
        // src の外 (tests/ benches/ examples/ の直下) は target の根 = file の置き場所は module にならない
        let mut segs: Vec<String> = match path.rsplit_once("/src/") {
            Some((_, in_src)) => in_src.split('/').map(|s| s.trim_end_matches(".rs").to_string()).collect(),
            None => Vec::new(),
        };
        if segs.last().is_some_and(|f| matches!(f.as_str(), "lib" | "main" | "mod")) {
            segs.pop();
        }
        segs.extend(d.module.split("::").filter(|m| !m.is_empty()).map(str::to_string));
        segs
    }

    /// 修飾つき呼び出しの出どころの crate (crate_key)。`crate::` / `self::` / `super::` / 修飾の出どころが
    /// 空 (同じ crate の module) なら呼び出し側の crate、workspace の crate 名ならその crate。分からなければ None。
    fn qual_crate(&self, cs: &CallSite) -> Option<String> {
        let own = || self.files.get(&cs.file).map(|f| f.0.clone());
        match cs.qual_root.as_str() {
            "" | "crate" | "self" | "super" => own(),
            r if self.local_roots.contains(&crate_key(r)) => Some(crate_key(r)),
            _ => None,
        }
    }

    fn is_local(&self, root: &str) -> bool {
        root.is_empty() || self.local_roots.contains(&crate_key(root))
    }
    /// 型の出どころが内側か。`use` していない prelude の型名 (Vec / String / ..) は std。
    fn is_local_ty(&self, root: &str, name: &str) -> bool {
        self.is_local(root) && !(root.is_empty() && PRELUDE_TYPES.contains(&name))
    }
    /// 確定先を rust-analyzer (= bake 済み kenning) の流儀に揃える (bench infer で RA と突き合わせた挙動)。
    /// trait 実装の method を
    /// - method 呼び `x.f()` で、具体的な型への impl なら → その impl の method のまま
    /// - method 呼びでも型引数そのもの / それを包む参照・ポインタへの impl (`impl<P> Tr for Pin<P>`) なら → trait の宣言
    /// - path 呼び `T::f()` なら → repo の trait は impl の method のまま (`ListEntry::as_raw`)、std / 依存の trait は
    ///   repo の外 (`BinaryDetection::default()` は std の Default::default を指す)
    ///
    /// trait が repo の外なら Err (= 呼び先は repo の外)、宣言を 1 つに絞れなければ Ok(None)。
    fn trait_decl_of(&self, d: &SymDef, name: &str, path_call: bool) -> Result<Option<EntityId>, ()> {
        if !d.impl_trait.name.is_empty() && !self.types.contains(&d.container) {
            return Ok(None); // repo で定義していない型への trait 実装 — その型の inherent method が先に当たり得る
        }
        if path_call && !d.impl_trait.name.is_empty() {
            return if self.is_local(&d.impl_trait.root) && self.traits.contains(&d.impl_trait.name) { Ok(Some(d.eid)) } else { Err(()) };
        }
        if d.impl_trait.name.is_empty() || !d.impl_blanket {
            return Ok(Some(d.eid));
        }
        if !self.is_local(&d.impl_trait.root) {
            return Err(());
        }
        let decls: Vec<&SymDef> = self
            .defs
            .get(name)
            .map(|v| v.iter().filter(|x| x.kind == K_METHOD && x.container == d.impl_trait.name && x.impl_trait.name.is_empty()).collect())
            .unwrap_or_default();
        Ok(match decls.as_slice() {
            [x] => Some(x.eid),
            _ => None,
        })
    }
    /// 修飾なしの `f()` から定義 `d` に届くか: 別の crate の fn は `use` していなければ呼べない。
    /// tests/ benches/ examples/ の直下の file は互いに別の crate (benches の `iter()` を tokio-stream の
    /// iter に確定した実例)。
    fn reachable_bare(&self, cs: &CallSite, d: &SymDef) -> bool {
        let (Some((caller_crate, caller_path)), Some((def_crate, def_path))) = (self.files.get(&cs.file), self.files.get(&d.file)) else {
            return true; // file の情報が無い (旧 index) なら従来どおり
        };
        if caller_path != def_path && target_root_file(caller_path) && target_root_file(def_path) {
            return false;
        }
        // tests/ benches/ examples/ 直下の file は自分の package の lib から見ても別の crate — 名前で use した時だけ
        // (グロブの use は追えないので確定しない)。
        if caller_path != def_path && target_root_file(caller_path) {
            return crate_key(&cs.qual_root) == *def_crate;
        }
        caller_crate == def_crate || crate_key(&cs.qual_root) == *def_crate
    }
    /// method 呼び `x.f()` を定義 `d` (型 `t` の method) に確定してよいか。Rust の method 解決で先に当たり得る
    /// 別の候補があれば確定しない:
    /// - `&T` への impl に同名 method がある (`fd.read()` の fd: &T は `impl Read for &T` が先)
    /// - d が trait 実装で、拡張 trait (全型向け impl) も同名を宣言している (`BufReader::consume` と
    ///   `AsyncBufReadExt::consume` — 受け手の形で拡張 trait が選ばれる)。inherent method は trait より優先なので可
    fn method_unshadowed(&self, cands: &[&SymDef], d: &SymDef, t: &str) -> bool {
        let ref_impl = cands.iter().any(|x| {
            let c = x.container.trim_start_matches('&').trim_start_matches("mut");
            let c = c.strip_prefix('\'').map(|r| r.trim_start_matches(|ch: char| ch.is_alphanumeric() || ch == '_')).unwrap_or(c);
            x.container.starts_with('&') && c == t
        });
        let ext = !d.impl_trait.name.is_empty()
            && cands.iter().any(|x| x.impl_trait.name.is_empty() && self.for_all_traits.contains(&x.container));
        !ref_impl && !ext
    }
    /// 型 `t` の関数 `f` (method なら `method` = true) の戻り値。ちょうど 1 つに絞れて、戻り値の出どころが
    /// 内側の時だけ。`Self` (と `Result<Self>` の Self) は t に読み替える。
    fn ret_of(&self, t: &str, f: &str, method: bool) -> Option<TyRef> {
        let ds: Vec<&SymDef> = self.defs.get(f)?.iter().filter(|d| d.container == t && (d.kind == K_METHOD || (!method && d.kind == K_FN))).collect();
        let [d] = ds.as_slice() else { return None };
        self.ret_from(d, t)
    }

    /// 定義 `d` の戻り値 (`Self` は `t` に読み替え。t が空 = 自由関数なら `Self` は推定しない)。
    fn ret_from(&self, d: &SymDef, t: &str) -> Option<TyRef> {
        let mut r = d.ret.clone();
        if r.name.is_empty() {
            return None;
        }
        if r.name == "Self" {
            if t.is_empty() {
                return None;
            }
            r.name = t.to_string();
            r.root = String::new();
        } else if !self.is_local_ty(&r.root, &r.name) && !matches!(r.name.as_str(), "Result" | "Option") {
            return None; // repo の外の型 (次の段の method は repo に無い)
        }
        if r.arg == "Self" {
            r.arg = t.to_string();
            r.arg_root = String::new();
        }
        Some(r)
    }
}

/// struct field の型の表: (struct 名, field 名) → 型 (同名 struct が複数 crate にあれば複数)。
pub(crate) type FieldTypes = HashMap<(String, String), Vec<TyRef>>;

/// 集めた field を field 表に書く (index / update 共通)。
pub(crate) fn insert_fields(field_t: &Table, fields: &[(EntityId, RawField)]) {
    for (file, f) in fields {
        field_t
            .insert()
            .set("strukt", f.strukt.as_str())
            .set("name", f.name.as_str())
            .set("ty", f.ty.name.as_str())
            .set("root", f.ty.root.as_str())
            .set("arg", f.ty.arg.as_str())
            .set("arg_root", f.ty.arg_root.as_str())
            .set("file", Value::Ref(*file))
            .commit()
            .unwrap();
    }
}

/// field 表 → FieldTypes (増分 update の再解決用)。旧 index で表が無ければ空。
pub(crate) fn load_field_types(field_t: Option<&Table>) -> FieldTypes {
    let mut m = FieldTypes::new();
    let Some(t) = field_t else { return m };
    for e in t.all().find().unwrap_or_default() {
        let er = t.entity(e);
        m.entry((txt(er.get("strukt")), txt(er.get("name")))).or_default().push(TyRef {
            name: txt(er.get("ty")),
            root: txt(er.get("root")),
            arg: txt(er.get("arg")),
            arg_root: txt(er.get("arg_root")),
        });
    }
    m
}

/// 1 呼び出し箇所を定義表に突き合わせて (解決先 eid, 信頼度) を返す。推測はしない。
pub(crate) fn resolve_call(cs: &CallSite, rz: &Resolver) -> (Option<EntityId>, u32) {
    let defs = rz.defs;
    let Some(all_cands) = defs.get(&cs.name) else {
        return (None, R_EXTERNAL); // 呼べる同名定義が index に無い = std / 依存 crate (解決不能)
    };
    // 代用品 (`cfg_not_*!`) と、外から見た cfg 付きの定義は確定先にしない (build の設定で別の実体になり得る)。
    // 外した候補は「確定先にしない」だけで、残りを一意にする根拠にはしない (外した方が本当の呼び先かも
    // しれない)。下の各段で「外した候補にも当てはまる物があれば確定しない」を見る。
    let (live, excluded): (Vec<SymDef>, Vec<SymDef>) = all_cands.iter().cloned().partition(|d| rz.live_for(cs.file, d));
    if !excluded.is_empty() && live.iter().all(|d| d.kind != K_FN && d.kind != K_METHOD) {
        return (None, R_AMBIG);
    }
    let shadowed = |f: &dyn Fn(&SymDef) -> bool| excluded.iter().any(f);
    let all_cands = &live;
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
        // 修飾の出どころが repo の外 (`use std::io::Error` の `Error::new`) なら、同名の自前の型の
        // new に確定してはいけない (ripgrep で 26 件の誤確定を RA との突き合わせで確認)。
        if !rz.is_local(&cs.qual_root) {
            return (None, R_EXTERNAL);
        }
        // `Self::` は現在の impl 型に読み替える。
        let target = if q == "Self" { cs.caller_container.as_str() } else { q };
        // (a) container 一致 = Type の associated fn / method。
        let by_container: Vec<&SymDef> = cands.iter().copied().filter(|d| d.container == target).collect();
        if shadowed(&|d| d.container == target) {
            return (None, R_AMBIG);
        }
        // `Trait::f(x)` / `<T as Trait>::f(..)` — trait 名で修飾した呼び出しは、引数 / 自分の型に応じた impl に
        // 解決される (RA も impl を指す)。宣言に確定すると外れる → 確定しない。
        if rz.traits.contains(target) && !by_container.is_empty() {
            return (None, R_AMBIG);
        }
        if let [d] = by_container.as_slice() {
            return match rz.trait_decl_of(d, &cs.name, true) {
                Ok(Some(e)) => (Some(e), R_QUALIFIED),
                Ok(None) => (None, R_AMBIG),
                Err(()) => (None, R_EXTERNAL),
            };
        }
        if by_container.len() > 1 {
            return (None, R_AMBIG); // 同名 Type が別 crate に複数 → 絞れない
        }
        // (b) `mod::fn` — 修飾の出どころの crate (`tokio::` / `crate::` / 名前の `use`) の中で、module の道順
        // (file の置き場所 + file 内の `mod x { }`) に修飾名を含む定義がちょうど 1 つなら、それ。子 module に
        // 実体を置いて `pub use` で出す形 (`time::sleep` の実体は `time/sleep.rs`) もここで拾う。同じ名前の
        // module が複数ある (`task` と `runtime::task`) なら、どちらの道順か決められないので確定しない。
        let in_crate = |d: &SymDef| rz.qual_crate(cs).is_none_or(|c| crate_key(&d.crate_) == c);
        let under_target = |d: &SymDef| in_crate(d) && rz.module_of(d).iter().any(|m| m == target);
        if !shadowed(&under_target)
            && let [d] = cands.iter().copied().filter(|d| under_target(d)).collect::<Vec<_>>().as_slice()
        {
            return (Some(d.eid), R_QUALIFIED);
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
            // 外の型への impl の中の `self.f()` は std の inherent method が先に当たる (`impl Kill for StdChild`)
            if !rz.is_local_ty(&cs.recv.root, &cs.caller_container) {
                return (None, R_METHOD);
            }
            if let [d] = by_self.as_slice()
                && !shadowed(&|x| x.container == cs.caller_container)
                && rz.method_unshadowed(&cands, d, &cs.caller_container)
            {
                return match rz.trait_decl_of(d, &cs.name, false) {
                    Ok(Some(e)) => (Some(e), R_UNIQUE),
                    Ok(None) => (None, R_METHOD),
                    Err(()) => (None, R_EXTERNAL),
                };
            }
        }
        // 受け手の型がソースに書いてあれば、その型の同名 method がちょうど 1 つの時だけ確定する。
        if let Some((t, wrapped)) = recv_type_of(cs, rz) {
            // trait 境界の受け手 (t が trait): 同名 method を宣言する**拡張 trait** (全型向けの impl を持つ) が
            // あれば、どちらが呼ばれるかは境界の外で決まる → 推定しない (AsyncBufRead / AsyncBufReadExt の consume)
            if rz.traits.contains(&t)
                && cands.iter().any(|d| d.container != t && d.impl_trait.name.is_empty() && rz.for_all_traits.contains(&d.container))
            {
                return (None, R_METHOD);
            }
            let by_ty: Vec<&SymDef> = cands.iter().copied().filter(|d| d.container == t).collect();
            if let [d] = by_ty.as_slice()
                && !(wrapped && (!d.impl_trait.name.is_empty() || rz.traits.contains(&t) || SMART_PTR_METHODS.contains(&cs.name.as_str())))
                && !shadowed(&|x| x.container == t)
                && rz.method_unshadowed(&cands, d, &t)
            {
                return match rz.trait_decl_of(d, &cs.name, false) {
                    Ok(Some(e)) => (Some(e), R_TYPED),
                    Ok(None) => (None, R_METHOD),
                    Err(()) => (None, R_EXTERNAL),
                };
            }
        }
        return (None, R_METHOD); // 候補どまり (名前一致は残るので `callers` には出る)
    }
    // 修飾なしの関数呼び。名前が束縛 (クロージャ / 引数) や外から `use` した関数なら repo の fn ではない
    // (`use std::os::unix::fs::symlink; symlink(..)` を同名の自前 fn に確定した実例)。
    if !rz.is_local(&cs.qual_root) {
        return (None, R_EXTERNAL);
    }
    // 修飾なしの `f()` は method を呼べない (呼ぶには `T::f` / `x.f()` が要る) — method は候補から外す。
    let fns: Vec<&SymDef> = cands.iter().copied().filter(|d| d.kind != K_METHOD).collect();
    if shadowed(&|d| d.kind != K_METHOD) {
        return (None, R_AMBIG);
    }
    // 同名がちょうど 1 つなら一意解決。複数でも、名前で `use` していない (出どころ = 空) 呼び出しから見て
    // 同じ file の定義がちょうど 1 つなら、それ (自分の module の item は glob の `use` より優先される。
    // tests/ の各 file の `fn setup()` / `fn walk()` のように、同名の helper を file ごとに持つのは普通)。
    match fns.as_slice() {
        [d] if rz.reachable_bare(cs, d) => (Some(d.eid), R_UNIQUE),
        [_] => (None, R_AMBIG),
        [] => (None, R_EXTERNAL),
        _ if cs.qual_root.is_empty() => match fns.iter().filter(|d| d.file == cs.file).collect::<Vec<_>>().as_slice() {
            [d] => (Some(d.eid), R_UNIQUE),
            _ => (None, R_AMBIG),
        },
        _ => (None, R_AMBIG),
    }
}

/// 受け手の型名。起点 (`Self` は caller の impl 型、`T::f()` 由来は f の戻り値) から連鎖の手順を 1 段ずつ
/// たどる。どの段でも「ちょうど 1 つに絞れる・出どころが repo の内側」でなければ None (推定しない)。
/// 戻り値の 2 つ目 = 受け手が smart pointer で包まれたまま (`Box::pin(x).f()`)。包んだ受け手では、包んだ側の
/// method が先に当たり得るので、呼び先は中身の inherent method に限る (resolve_call で見る)。中身が trait
/// (trait の中の `Pin::new(self)`) なら、Pin が実装する trait の method に当たる (tokio の consume で RA と照合)。
fn recv_type_of(cs: &CallSite, rz: &Resolver) -> Option<(String, bool)> {
    let r = &cs.recv;
    // 直前の段が関数の戻り値なら、その型 (`?` で中身を取るため)。
    let mut last_ret: Option<TyRef> = None;
    let mut t = if let Some(q) = r.ty.strip_prefix(FREE_FN_RECV) {
        let ret = free_fn_ret(cs, q, rz)?;
        let t = ret.name.clone();
        last_ret = Some(ret);
        t
    } else {
        if r.ty.is_empty() || !rz.is_local_ty(&r.root, &r.ty) {
            return None;
        }
        let t = if r.ty == "Self" { cs.caller_container.clone() } else { r.ty.clone() };
        if t.is_empty() {
            return None;
        }
        if r.via_fn.is_empty() {
            t
        } else {
            let ret = rz.ret_of(&t, &r.via_fn, false)?;
            let t = ret.name.clone();
            last_ret = Some(ret);
            t
        }
    };
    if r.via_try {
        t = unwrap_try(last_ret.take()?, rz)?;
    }
    let mut wrapped = false;
    for s in r.chain.split(' ').filter(|s| !s.is_empty()) {
        if s == "?" {
            t = unwrap_try(last_ret.take()?, rz)?;
        } else if let Some(w) = s.strip_prefix("w:") {
            if rz.types.contains(w) {
                return None; // 自前の同名型 (std の smart pointer ではない)
            }
            if last_ret.as_ref().is_some_and(|r| !rz.is_local_ty(&r.root, &r.name)) {
                return None; // 包む前が std の Result / Option のまま
            }
            last_ret = None;
            wrapped = true;
        } else if let Some(f) = s.strip_prefix("f:") {
            wrapped = false;
            last_ret = None;
            let tys = rz.fields.get(&(t.clone(), f.to_string()))?;
            let [ty] = tys.as_slice() else { return None }; // 同名 struct が複数 → 絞れない
            if ty.name.is_empty() || !rz.is_local_ty(&ty.root, &ty.name) {
                return None;
            }
            t = ty.name.clone();
        } else if let Some(m) = s.strip_prefix("m:") {
            if wrapped && matches!(m, "as_mut" | "as_ref") {
                continue; // `Pin<Box<T>>::as_mut()` / `Box::as_ref()` — 中身への参照 (まだ包まれている扱い)
            }
            if wrapped && SMART_PTR_METHODS.contains(&m) {
                return None;
            }
            let ret = rz.ret_of(&t, m, true)?;
            t = ret.name.clone();
            last_ret = Some(ret);
            wrapped = false;
        } else {
            return None;
        }
    }
    // 連鎖の最後が戻り値の Result / Option / Vec 等 (`?` を当てていない) なら std の型
    let ext_tail = last_ret.as_ref().is_some_and(|r| !rz.is_local_ty(&r.root, &r.name));
    (!ext_tail).then_some((t, wrapped))
}

/// 受け手が自由関数の戻り値 (`let s = time::sleep(..); s.reset()`) — 呼び先を関数呼びと同じ規則で 1 つに絞れた時
/// だけ、その定義に書いてある戻り値の型。外から `use` した関数や同名の自由関数が複数なら推定しない。
fn free_fn_ret(cs: &CallSite, qualifier: &str, rz: &Resolver) -> Option<TyRef> {
    let r = &cs.recv;
    let probe = CallSite {
        caller: cs.caller,
        caller_container: cs.caller_container.clone(),
        file: cs.file,
        rel_path: cs.rel_path.clone(),
        name: r.via_fn.clone(),
        qualifier: (!qualifier.is_empty()).then(|| qualifier.to_string()),
        is_method: false,
        self_recv: false,
        as_value: false,
        in_macro: false,
        recv: Recv::default(),
        qual_root: r.root.clone(),
        line: cs.line,
        col: cs.col,
    };
    let (Some(e), _) = resolve_call(&probe, rz) else { return None };
    let d = rz.defs.get(&r.via_fn)?.iter().find(|d| d.eid == e && d.kind == K_FN)?;
    rz.ret_from(d, "")
}

/// `?` を当てた後の型: 戻り値が `Result<T, _>` / `Option<T>` の T (出どころが内側の時だけ)。
fn unwrap_try(ret: TyRef, rz: &Resolver) -> Option<String> {
    (matches!(ret.name.as_str(), "Result" | "Option") && !ret.arg.is_empty() && rz.is_local(&ret.arg_root)).then_some(ret.arg)
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
        .set("line", cs.line)
        .set("recv_ty", cs.recv.ty.as_str())
        .set("recv_fn", cs.recv.via_fn.as_str())
        .set("recv_try", cs.recv.via_try as u32)
        .set("recv_root", cs.recv.root.as_str())
        .set("recv_chain", cs.recv.chain.as_str())
        .set("has_chain", !cs.recv.chain.is_empty() as u32)
        .set("qual_root", cs.qual_root.as_str());
    if cs.caller != 0 {
        ins = ins.set("caller", Value::Ref(cs.caller)); // 0 = item 直下 (関数の外) → 未 set
    }
    if let Some(eid) = target {
        ins = ins.set("callee_sym", Value::Ref(eid));
    }
    ins.commit().unwrap();
}

/// syn ヒューリスティックで callee を解決して挿入。res を返す。
pub(crate) fn insert_call(call_t: &Table, cs: &CallSite, rz: &Resolver) -> u32 {
    let (target, res) = resolve_call(cs, rz);
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
            ret: TyRef { name: txt(er.get("ret_ty")), root: txt(er.get("ret_root")), arg: txt(er.get("ret_arg")), arg_root: txt(er.get("ret_arg_root")) },
            impl_trait: TyRef { name: txt(er.get("impl_trait")), root: txt(er.get("impl_trait_root")), ..Default::default() },
            impl_blanket: num(er.get("impl_blanket")) == 1,
            file: ref_of(er.get("file")),
            gated: num(er.get("gated")) == 1,
            cond: num(er.get("cond")) == 1,
        });
    }
    defs
}

/// 1 ファイルの sym / call / impl / file 行を全消去。消えた sym の名前を `affected` に集める
/// (他ファイルからの incoming edge を後で再解決するため)。impl_t は旧 index で無いこともある。
pub(crate) fn purge_file(sym_t: &Table, call_t: &Table, file_t: &Table, impl_t: Option<&Table>, field_t: Option<&Table>, file_eid: EntityId, affected: &mut HashSet<String>) {
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
    // field の型が変わると、それを通る連鎖 (`self.cfg.run()`) の解決が変わる → field 名も影響名に。
    if let Some(ft) = field_t {
        for fe in ft.all().where_ref("file", file_eid).find().unwrap_or_default() {
            affected.insert(txt(ft.entity(fe).get("name")));
            ft.entity(fe).delete().unwrap();
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
/// file 内の `use` (inline mod の中も) から「名前 → 出どころ (先頭 seg)」を作る。glob は拾えない (空 = ローカル扱い)。
/// `use` の木 1 本を「名前 → (出どころ, 元の名前)」に展開する。
pub(crate) fn use_tree_imports(t: &syn::UseTree, root: Option<&str>, out: &mut HashMap<String, (String, String)>) {
    match t {
        syn::UseTree::Path(p) => {
            let r = root.map(str::to_string).unwrap_or_else(|| p.ident.to_string());
            use_tree_imports(&p.tree, Some(&r), out);
        }
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            if name != "self" {
                out.insert(name.clone(), (root.map(str::to_string).unwrap_or_else(|| name.clone()), name));
            }
        }
        syn::UseTree::Rename(r) => {
            let orig = r.ident.to_string();
            out.insert(r.rename.to_string(), (root.map(str::to_string).unwrap_or_else(|| orig.clone()), orig));
        }
        syn::UseTree::Group(g) => g.items.iter().for_each(|i| use_tree_imports(i, root, out)),
        syn::UseTree::Glob(_) => {}
    }
}

pub(crate) fn file_imports(items: &[syn::Item]) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    for it in items {
        match it {
            syn::Item::Use(u) => use_tree_imports(&u.tree, None, &mut out),
            syn::Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    out.extend(file_imports(inner));
                }
            }
            // `cfg_rt! { mod x { use std::cell::Cell; .. } }` — item を包むマクロの中の use も効く
            syn::Item::Macro(m) if !m.mac.path.is_ident("macro_rules") => {
                if let Ok(items) = m.mac.parse_body_with(parse_items::<syn::Item>) {
                    out.extend(file_imports(&items));
                }
            }
            _ => {}
        }
    }
    out
}

pub(crate) fn extract_file(src: &str) -> Option<FileFacts> {
    let file = syn::parse_file(src).ok()?;
    let mut out = FileFacts { loc: src.lines().count() as u32, hash: hash_u32(src), syms: Vec::new(), impls: Vec::new(), item_calls: Vec::new(), fields: Vec::new(), gated_mods: Vec::new(), live_mods: Vec::new(), cond_mods: Vec::new(), mod_names: Vec::new() };
    // file 冒頭の inner attribute (`#![cfg(test)]`) は file 全体に効く。per-file parse では親の
    // `#[cfg(test)] mod x;` が見えないので、これを見ないと test 専用 file の helper が
    // 「test でない symbol」として出てしまう (`tests <name>` と `search test:1` が取りこぼす)。
    let mut ctx = Ctx { module: Vec::new(), in_test: file.attrs.iter().any(is_cfg_test), container: String::new(), in_trait_impl: false, impl_trait: TyRef::default(), impl_blanket: false, impl_self_root: String::new(), neg_gate: false, cond: false, impl_generics: Vec::new(), impl_bounds: HashMap::new(), imports: std::rc::Rc::new(file_imports(&file.items)) };
    walk_items(&file.items, &mut ctx, &mut out);
    let live = std::mem::take(&mut out.live_mods);
    out.gated_mods.retain(|m| !live.contains(m)); // 両方の枝で宣言 = 出し分けであって代用品ではない
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
    // md の code block (``` / ~~~) の中の `# ` はシェルのコメント等で、見出しではない。開いた印で閉じる。
    let mut fence: Option<&str> = None;
    for (i, raw) in src.lines().enumerate() {
        let ln = (i + 1) as u32;
        match lang {
            LANG_MD => {
                let t = raw.trim_start();
                if let Some(mark) = ["```", "~~~"].into_iter().find(|m| t.starts_with(m)) {
                    fence = match fence {
                        Some(open) if open == mark => None,
                        None => Some(mark),
                        keep => keep,
                    };
                    continue;
                }
                if fence.is_some() {
                    continue;
                }
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
