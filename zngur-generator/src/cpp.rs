use indoc::indoc;
use itertools::Itertools;
use std::{
    collections::HashMap,
    fmt::{Display, Write},
    iter, panic,
};
use zngur_def::{Mutability, RustTrait, ZngurField, ZngurMethodReceiver};

use crate::{ZngurWellknownTraitData, rust::IntoCpp};

#[derive(Debug)]
pub struct CppPath(pub Vec<String>);

impl CppPath {
    fn namespace(&self) -> &[String] {
        self.0.split_last().unwrap().1
    }

    fn emit_in_namespace(
        &self,
        state: &mut State,
        f: impl FnOnce(&mut State) -> std::fmt::Result,
    ) -> std::fmt::Result {
        for p in self.namespace() {
            writeln!(state, "namespace {} {{", p)?;
        }
        f(state)?;
        for _ in self.namespace() {
            writeln!(state, "}}")?;
        }
        Ok(())
    }

    fn name(&self) -> &str {
        self.0.split_last().unwrap().0
    }

    fn need_header(&self) -> bool {
        self.0.first().map(|x| x.as_str()) == Some("rust")
            && self.0 != ["rust", "Unit"]
            && self.0 != ["rust", "Ref"]
            && self.0 != ["rust", "RefMut"]
    }

    pub(crate) fn from_rust_path(path: &[String]) -> CppPath {
        CppPath(
            iter::once("rust")
                .chain(path.iter().map(|x| x.as_str()))
                .map(cpp_handle_keyword)
                .map(|x| x.to_owned())
                .collect(),
        )
    }
}

impl<const N: usize> From<[&str; N]> for CppPath {
    fn from(value: [&str; N]) -> Self {
        CppPath(value.iter().map(|x| x.to_string()).collect())
    }
}

impl From<&str> for CppPath {
    fn from(value: &str) -> Self {
        let value = value.trim();
        CppPath(value.split("::").map(|x| x.to_owned()).collect())
    }
}

impl Display for CppPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "::{}", self.0.iter().join("::"))
    }
}

#[derive(Debug)]
pub struct CppType {
    pub path: CppPath,
    pub generic_args: Vec<CppType>,
}

impl CppType {
    pub fn into_ref(self) -> CppType {
        CppType {
            path: CppPath::from("rust::Ref"),
            generic_args: vec![self],
        }
    }

    fn emit_specialization_decl(&self, state: &mut State) -> std::fmt::Result {
        if self.generic_args.is_empty() {
            write!(state, "struct {}", self.path.name())?;
        } else {
            write!(
                state,
                "template<> struct {}< {} >",
                self.path.name(),
                self.generic_args.iter().join(", ")
            )?;
        }
        Ok(())
    }

    fn emit_header(&self, state: &mut State) -> std::fmt::Result {
        for x in &self.generic_args {
            x.emit_header(state)?;
        }
        if !self.path.need_header() {
            return Ok(());
        }
        self.path.emit_in_namespace(state, |state| {
            if !self.generic_args.is_empty() {
                writeln!(state, "template<typename ...T>")?;
            }
            writeln!(state, "struct {};", self.path.name())
        })
    }
}

impl Display for CppType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path)?;
        if !self.generic_args.is_empty() {
            write!(f, "< {} >", self.generic_args.iter().join(", "))?;
        }
        Ok(())
    }
}

fn split_string(input: &str) -> impl Iterator<Item = String> {
    let mut parts = Vec::new();
    let mut current_part = String::new();
    let mut parentheses_count = 0;

    for c in input.chars() {
        match c {
            ',' if parentheses_count == 0 => {
                parts.push(current_part.clone());
                current_part.clear();
            }
            '<' => {
                parentheses_count += 1;
                current_part.push(c);
            }
            '>' => {
                parentheses_count -= 1;
                current_part.push(c);
            }
            _ => {
                current_part.push(c);
            }
        }
    }

    if !current_part.is_empty() {
        parts.push(current_part);
    }

    parts.into_iter()
}

impl From<&str> for CppType {
    fn from(value: &str) -> Self {
        let value = value.trim();
        match value.split_once('<') {
            None => CppType {
                path: CppPath::from(value),
                generic_args: vec![],
            },
            Some((path, generics)) => {
                let generics = generics.strip_suffix('>').unwrap();
                CppType {
                    path: CppPath::from(path),
                    generic_args: split_string(generics).map(|x| CppType::from(&*x)).collect(),
                }
            }
        }
    }
}

struct State {
    text: String,
    panic_to_exception: bool,
}

impl State {
    fn panic_handler(&self) -> String {
        if self.panic_to_exception {
            r#"
            if (__zngur_detect_panic()) {
                __zngur_take_panic();
                throw ::rust::Panic{};
            }
            "#
            .to_owned()
        } else {
            "".to_owned()
        }
    }

    fn remove_no_except_in_panic(&mut self) {
        if self.panic_to_exception {
            self.text = self.text.replace(" noexcept ", " ");
        }
    }
}

impl Write for State {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.text += s;
        Ok(())
    }
}

#[derive(Debug)]
pub struct CppTraitMethod {
    pub name: String,
    pub rust_link_name: String,
    pub inputs: Vec<CppType>,
    pub output: CppType,
}

#[derive(Debug)]
pub struct CppFnSig {
    pub rust_link_name: String,
    pub inputs: Vec<CppType>,
    pub output: CppType,
}

impl CppFnSig {
    fn emit_rust_link(&self, state: &mut State) -> std::fmt::Result {
        write!(state, "void {}(", self.rust_link_name)?;
        for n in 0..self.inputs.len() {
            write!(state, "uint8_t* i{n},")?;
        }
        write!(state, "uint8_t* o)")?;
        Ok(())
    }

    fn emit_rust_link_decl(&self, state: &mut State) -> std::fmt::Result {
        self.emit_rust_link(state)?;
        writeln!(state, " noexcept ;")?;
        Ok(())
    }

    fn emit_zngur_dyn_api_field(&self, state: &mut State) -> std::fmt::Result {
        write!(
            state,
            r###"decltype(&::{link_name}) {link_name};"###,
            link_name = self.rust_link_name
        )?;
        Ok(())
    }

    fn emit_cpp_header(&self, state: &mut State, fn_name: &str) -> std::fmt::Result {
        let CppFnSig {
            inputs,
            output,
            rust_link_name: _,
        } = self;
        writeln!(
            state,
            "{output} {fn_name}({dyn_api_def}{input_defs}) noexcept ;",
            dyn_api_def = if inputs.is_empty() {
                "__zngur_dyn_api* api"
            } else {
                "__zngur_dyn_api* api, "
            },
            input_defs = inputs
                .iter()
                .enumerate()
                .map(|(n, ty)| format!("{ty} i{n}"))
                .join(", "),
        )
    }

    fn emit_cpp_def(&self, state: &mut State, fn_name: &str) -> std::fmt::Result {
        let CppFnSig {
            inputs,
            output,
            rust_link_name,
        } = self;
        writeln!(
            state,
            "inline {output} {fn_name}({dyn_api_def}{input_defs}) noexcept {{
            {output} o{{}};
            {deinits}
            api->{rust_link_name}({input_args}::rust::__zngur_internal_data_ptr(o));
            {panic_handler}
            ::rust::__zngur_internal_assume_init(o);
            return o;
        }}",
            dyn_api_def = if inputs.is_empty() {
                "__zngur_dyn_api* api"
            } else {
                "__zngur_dyn_api* api, "
            },
            input_defs = inputs
                .iter()
                .enumerate()
                .map(|(n, ty)| format!("{ty} i{n}"))
                .join(", "),
            input_args = (0..inputs.len())
                .map(|n| format!("::rust::__zngur_internal_data_ptr(i{n}), "))
                .join(""),
            panic_handler = state.panic_handler(),
            deinits = (0..inputs.len())
                .map(|n| format!("::rust::__zngur_internal_assume_deinit(i{n});"))
                .join("\n"),
        )
    }
}

pub struct CppFnDefinition {
    pub name: CppPath,
    pub sig: CppFnSig,
}

pub struct CppExportedFnDefinition {
    pub name: String,
    pub sig: CppFnSig,
}

pub struct CppExportedImplDefinition {
    pub tr: Option<CppType>,
    pub ty: CppType,
    pub methods: Vec<(String, CppFnSig)>,
}

impl CppFnDefinition {
    fn emit_cpp_def(&self, state: &mut State) -> std::fmt::Result {
        self.name.emit_in_namespace(state, |state| {
            self.sig.emit_cpp_def(state, self.name.name())
        })
    }
}

#[derive(Debug)]
pub struct CppMethod {
    pub name: String,
    pub kind: ZngurMethodReceiver,
    pub sig: CppFnSig,
}

#[derive(Debug)]
pub struct BuildFromFunction {
    pub sig: CppFnSig,
}

#[derive(Debug)]
pub enum CppTraitDefinition {
    Fn {
        sig: CppFnSig,
    },
    Normal {
        as_ty: CppType,
        methods: Vec<CppTraitMethod>,
        link_name: String,
        link_name_ref: String,
    },
}

impl CppTraitDefinition {
    fn emit_rust_links(&self, state: &mut State) -> std::fmt::Result {
        match self {
            CppTraitDefinition::Fn {
                sig:
                    CppFnSig {
                        rust_link_name,
                        inputs,
                        output: _,
                    },
            } => {
                writeln!(
                    state,
                    "void {rust_link_name}(uint8_t *data, void destructor(uint8_t *),
                void call(uint8_t *, {} uint8_t *),
                uint8_t *o);",
                    (0..inputs.len()).map(|_| "uint8_t *, ").join(" ")
                )?;
            }
            CppTraitDefinition::Normal {
                link_name,
                link_name_ref,
                ..
            } => {
                writeln!(
                    state,
                    "void {link_name}(uint8_t *data, void destructor(uint8_t *), uint8_t *o);"
                )?;
                writeln!(state, "void {link_name_ref}(uint8_t *data, uint8_t *o);")?;
            }
        }
        Ok(())
    }

    fn emit(&self, state: &mut State) -> std::fmt::Result {
        let CppTraitDefinition::Normal {
            as_ty,
            methods,
            link_name: _,
            link_name_ref: _,
        } = self
        else {
            return Ok(());
        };
        as_ty.path.emit_in_namespace(state, |state| {
            as_ty.emit_specialization_decl(state)?;
            write!(
                state,
                r#"{{
    public:
        virtual ~{}() {{}}
    "#,
                as_ty.path.name(),
            )?;
            for method in methods {
                write!(
                    state,
                    r#"
            virtual {output} {name}({input}) = 0;
    "#,
                    output = method.output,
                    name = method.name,
                    input = method
                        .inputs
                        .iter()
                        .enumerate()
                        .map(|(n, x)| format!("{x} i{n}"))
                        .join(", "),
                )?;
            }
            write!(
                state,
                r#"
    }};
    "#,
            )
        })
    }

    fn emit_cpp(&self, state: &mut State) -> std::fmt::Result {
        match self {
            CppTraitDefinition::Fn { .. } => (),
            CppTraitDefinition::Normal {
                as_ty,
                methods,
                link_name: _,
                link_name_ref: _,
            } => {
                for method in methods {
                    write!(state, "void {}(uint8_t* data", method.rust_link_name)?;
                    for arg in 0..method.inputs.len() {
                        write!(state, ", uint8_t* i{arg}")?;
                    }
                    writeln!(state, ", uint8_t* o) {{")?;
                    writeln!(
                        state,
                        "   {as_ty}* data_typed = reinterpret_cast< {as_ty}* >(data);"
                    )?;
                    write!(
                        state,
                        "   {} oo = data_typed->{}({});",
                        method.output,
                        method.name,
                        method
                            .inputs
                            .iter()
                            .enumerate()
                            .map(|(n, ty)| {
                                format!("::rust::__zngur_internal_move_from_rust< {ty} >(i{n})")
                            })
                            .join(", ")
                    )?;
                    writeln!(state, "   ::rust::__zngur_internal_move_to_rust(o, oo);")?;
                    writeln!(state, "}}")?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CppLayoutPolicy {
    StackAllocated {
        size: usize,
        align: usize,
    },
    HeapAllocated {
        size_fn: String,
        alloc_fn: String,
        free_fn: String,
    },
    OnlyByRef,
}

#[derive(Debug)]
pub struct CppTypeDefinition {
    pub ty: CppType,
    pub layout: CppLayoutPolicy,
    pub methods: Vec<CppMethod>,
    pub constructors: Vec<CppFnSig>,
    pub fields: Vec<ZngurField>,
    pub from_trait: Option<RustTrait>,
    pub from_trait_ref: Option<RustTrait>,
    pub wellknown_traits: Vec<ZngurWellknownTraitData>,
    pub cpp_value: Option<(String, String)>,
    pub cpp_ref: Option<String>,
}

impl Default for CppTypeDefinition {
    fn default() -> Self {
        Self {
            ty: CppType::from("fill::me::you::forgot::it"),
            layout: CppLayoutPolicy::OnlyByRef,
            methods: vec![],
            constructors: vec![],
            fields: vec![],
            wellknown_traits: vec![],
            from_trait: None,
            from_trait_ref: None,
            cpp_value: None,
            cpp_ref: None,
        }
    }
}

impl CppTypeDefinition {
    fn emit_field_specialization(&self, state: &mut State) -> std::fmt::Result {
        for field_kind in ["FieldOwned", "FieldRef", "FieldRefMut"] {
            writeln!(
                state,
                r#"
    namespace rust {{
    template<size_t OFFSET>
    struct {field_kind}< {ty}, OFFSET > {{
                "#,
                ty = self.ty,
            )?;
            for field in &self.fields {
                writeln!(
                    state,
                    "[[no_unique_address]] {field_kind}<{}, OFFSET + {}> {};",
                    field.ty.into_cpp(),
                    field.offset,
                    cpp_handle_field_name(&field.name),
                )?;
            }
            for method in &self.methods {
                if let ZngurMethodReceiver::Ref(m) = method.kind {
                    if m == Mutability::Mut && field_kind == "FieldRef" {
                        continue;
                    }
                    let CppFnSig {
                        rust_link_name: _,
                        inputs,
                        output,
                    } = &method.sig;
                    writeln!(
                        state,
                        "{output} {fn_name}({input_defs}) const noexcept ;",
                        fn_name = &method.name,
                        input_defs = inputs
                            .iter()
                            .skip(1)
                            .enumerate()
                            .map(|(n, ty)| format!("{ty} i{n}"))
                            .join(", "),
                    )?;
                }
            }
            writeln!(state, "}};\n}}")?;
        }
        Ok(())
    }

    fn emit_ref_specialization(&self, state: &mut State) -> std::fmt::Result {
        for ref_kind in ["RefMut", "Ref"] {
            let is_unsized = self
                .wellknown_traits
                .contains(&ZngurWellknownTraitData::Unsized);
            if self.ty.path.to_string() == "::rust::Str" && ref_kind == "Ref" {
                writeln!(
                    state,
                    r#"
    auto operator""_rs(const char* input, size_t len) -> ::rust::Ref<::rust::Str>;
"#,
                )?;
            }
            if is_unsized {
                writeln!(
                    state,
                    r#"
namespace rust {{
template<>
struct {ref_kind}< {ty} > {{
    {ref_kind}() {{
        data = {{0, 0}};
    }}
private:
    ::std::array<size_t, 2> data;
    friend uint8_t* ::rust::__zngur_internal_data_ptr< ::rust::{ref_kind}< {ty} > >(const ::rust::{ref_kind}< {ty} >& t) noexcept ;
    friend ::rust::ZngurPrettyPrinter< ::rust::{ref_kind}< {ty} > >;
"#,
                    ty = self.ty,
                )?;
            } else {
                writeln!(
                    state,
                    r#"
namespace rust {{
template<>
struct {ref_kind}< {ty} > {{
    {ref_kind}() {{
        data = 0;
    }}
"#,
                    ty = self.ty,
                )?;
                if !matches!(self.layout, CppLayoutPolicy::OnlyByRef) {
                    writeln!(
                        state,
                        r#"
    {ref_kind}(const {ty}& t) {{
        ::rust::__zngur_internal_check_init< {ty} >(t);
        data = reinterpret_cast<size_t>(__zngur_internal_data_ptr(t));
    }}
"#,
                        ty = self.ty,
                    )?;
                }
                for field in &self.fields {
                    writeln!(
                        state,
                        "[[no_unique_address]] ::rust::Field{ref_kind}<{}, {}> {};",
                        field.ty.into_cpp(),
                        field.offset,
                        cpp_handle_field_name(&field.name),
                    )?;
                }
                writeln!(
                    state,
                    r#"
private:
    size_t data;
    friend uint8_t* ::rust::__zngur_internal_data_ptr< ::rust::{ref_kind}< {ty} > >(const ::rust::{ref_kind}< {ty} >& t) noexcept ;
    friend ::rust::ZngurPrettyPrinter< ::rust::{ref_kind}< {ty} > >;
"#,
                    ty = self.ty,
                )?;
            }
            writeln!(state, "public:")?;
            if ref_kind == "Ref" {
                writeln!(
                    state,
                    r#"
    Ref(RefMut< {ty} > rm) {{
        data = rm.data;
    }}
    "#,
                    ty = self.ty,
                )?;
                if !is_unsized {
                    writeln!(
                        state,
                        r#"
    template<size_t OFFSET>
    Ref(const FieldOwned< {ty}, OFFSET >& f) {{
        data = reinterpret_cast<size_t>(&f) + OFFSET;
    }}

    template<size_t OFFSET>
    Ref(const FieldRef< {ty}, OFFSET >& f) {{
        data = *reinterpret_cast<const size_t*>(&f) + OFFSET;
    }}

    template<size_t OFFSET>
    Ref(const FieldRefMut< {ty}, OFFSET >& f) {{
        data = *reinterpret_cast<const size_t*>(&f) + OFFSET;
    }}
    "#,
                        ty = self.ty,
                    )?;
                }
            } else {
                writeln!(
                    state,
                    r#"
    friend Ref< {ty} >;
    "#,
                    ty = self.ty,
                )?;
                if !is_unsized {
                    writeln!(
                        state,
                        r#"
    template<size_t OFFSET>
    RefMut(const FieldOwned< {ty}, OFFSET >& f) {{
        data = reinterpret_cast<size_t>(&f) + OFFSET;
    }}

    template<size_t OFFSET>
    RefMut(const FieldRefMut< {ty}, OFFSET >& f) {{
        data = *reinterpret_cast<const size_t*>(&f) + OFFSET;
    }}
    "#,
                        ty = self.ty,
                    )?;
                }
            }
            match &self.from_trait_ref {
                Some(RustTrait::Fn { inputs, output, .. }) => {
                    let as_std_function = format!(
                        "::std::function< {}({})>",
                        output.into_cpp(),
                        inputs.iter().map(|x| x.into_cpp()).join(", ")
                    );
                    writeln!(
                        state,
                        r#"
inline {ty}({as_std_function} f);
"#,
                        ty = self.ty.path.name(),
                    )?;
                }
                Some(tr @ RustTrait::Normal { .. }) => {
                    let tr = tr.into_cpp();
                    writeln!(
                        state,
                        r#"
            inline {ref_kind}({tr}& arg);
            "#,
                    )?;
                }
                None => (),
            }
            if let Some((rust_link_name, cpp_ty)) = &self.cpp_value {
                writeln!(
                    state,
                    r#"
                inline {cpp_ty}& cpp() {{
                    return (*{rust_link_name}(reinterpret_cast<uint8_t*>(data))).as_cpp< {cpp_ty} >();
                }}"#
                )?;
            }
            if let Some(cpp_ty) = &self.cpp_ref {
                writeln!(
                    state,
                    r#"
                inline {cpp_ty}& cpp() {{
                    return *reinterpret_cast< {cpp_ty}* >(data);
                }}"#
                )?;
                writeln!(
                    state,
                    r#"
                inline {ref_kind}(const {cpp_ty}& t) : data(reinterpret_cast<size_t>(&t)) {{}}"#
                )?;
            }
            for method in &self.methods {
                if let ZngurMethodReceiver::Ref(m) = method.kind {
                    if m == Mutability::Mut && ref_kind == "Ref" {
                        continue;
                    }
                    let CppFnSig {
                        rust_link_name: _,
                        inputs,
                        output,
                    } = &method.sig;
                    writeln!(
                        state,
                        "{output} {fn_name}({dyn_api_def}{input_defs}) const noexcept ;",
                        dyn_api_def = if inputs.len().saturating_sub(1) == 0 {
                            "__zngur_dyn_api* api"
                        } else {
                            "__zngur_dyn_api* api, "
                        },
                        fn_name = &method.name,
                        input_defs = inputs
                            .iter()
                            .skip(1)
                            .enumerate()
                            .map(|(n, ty)| format!("{ty} i{n}"))
                            .join(", "),
                    )?;
                }
            }
            if self.ty.path.to_string() == "::rust::Str" && ref_kind == "Ref" {
                writeln!(
                    state,
                    r#"
    friend auto ::operator""_rs(const char* input, size_t len) -> ::rust::Ref<::rust::Str>;
}};
"#,
                )?;
            } else {
                writeln!(state, "}};")?;
            }
            writeln!(
                state,
                r#"
template<>
inline uint8_t* __zngur_internal_data_ptr< {ref_kind} < {ty} > >(const {ref_kind}< {ty} >& t) noexcept {{
    return const_cast<uint8_t*>(reinterpret_cast<const uint8_t*>(&t.data));
}}

template<>
inline void __zngur_internal_assume_init< {ref_kind} < {ty} > >({ref_kind}< {ty} >&) noexcept {{
}}

template<>
inline void __zngur_internal_check_init< {ref_kind} < {ty} > >(const {ref_kind}< {ty} >&) noexcept {{
}}

template<>
inline void __zngur_internal_assume_deinit< {ref_kind} < {ty} > >({ref_kind}< {ty} >&) noexcept {{
}}

template<>
inline size_t __zngur_internal_size_of< {ref_kind} < {ty} > >() noexcept {{
    return {size};
}}
}}"#,
                ty = self.ty,
                size = if is_unsized { 16 } else { 8 },
            )?;
            if self.ty.path.to_string() == "::rust::Str" && ref_kind == "Ref" {
                writeln!(
                    state,
                    r#"
inline ::rust::Ref<::rust::Str> operator""_rs(const char* input, size_t len) {{
    ::rust::Ref<::rust::Str> o;
    o.data[0] = reinterpret_cast<size_t>(input);
    o.data[1] = len;
    return o;
}}
                    "#,
                )?;
            }
        }
        Ok(())
    }

    fn emit(&self, state: &mut State) -> std::fmt::Result {
        let is_copy = self
            .wellknown_traits
            .contains(&ZngurWellknownTraitData::Copy);
        writeln!(
            state,
            r#"
namespace rust {{
    template<>
    inline uint8_t* __zngur_internal_data_ptr< {ty} >(const {ty}& t) noexcept ;
    template<>
    inline void __zngur_internal_check_init< {ty} >(const {ty}& t) noexcept ;
    template<>
    inline void __zngur_internal_assume_init< {ty} >({ty}& t) noexcept ;
    template<>
    inline void __zngur_internal_assume_deinit< {ty} >({ty}& t) noexcept ;
    template<>
    inline size_t __zngur_internal_size_of< {ty} >() noexcept ;
}}"#,
            ty = self.ty,
        )?;
        self.ty.path.emit_in_namespace(state, |state| {
            if self.ty.path.0 == ["rust", "Unit"] {
                write!(
                    state,
                    "template<> struct Tuple<> {{ ::std::array< ::uint8_t, 1> data; }};"
                )?;
                return Ok(());
            } else {
                self.ty.emit_specialization_decl(state)?;
            }
            match self.layout {
                CppLayoutPolicy::OnlyByRef => {
                    writeln!(
                        state,
                        r#"
{{
public:
    {ty}() = delete;
    "#,
                        ty = self.ty.path.name(),
                    )?;
                }
                CppLayoutPolicy::HeapAllocated { .. } | CppLayoutPolicy::StackAllocated { .. } => {
                    match self.layout {
                        CppLayoutPolicy::StackAllocated { size, align } => {
                            writeln!(
                                state,
                                r#"
{{
private:
    alignas({align}) mutable ::std::array<uint8_t, {size}> data;
            "#,
                            )?;
                        }
                        CppLayoutPolicy::HeapAllocated { .. } => {
                            writeln!(
                                state,
                                r#"
{{
private:
    uint8_t* data;
            "#,
                            )?;
                        }
                        CppLayoutPolicy::OnlyByRef => unreachable!(),
                    }
                    writeln!(
                        state,
                        r#"
    friend uint8_t* ::rust::__zngur_internal_data_ptr< {ty} >(const {ty}& t) noexcept ;
    friend void ::rust::__zngur_internal_check_init< {ty} >(const {ty}& t) noexcept ;
    friend void ::rust::__zngur_internal_assume_init< {ty} >({ty}& t) noexcept ;
    friend void ::rust::__zngur_internal_assume_deinit< {ty} >({ty}& t) noexcept ;
    friend ::rust::ZngurPrettyPrinter< {ty} >;
"#,
                        ty = self.ty,
                    )?;
                    if self.ty.path.to_string() == "::rust::Bool" {
                        assert_eq!(
                            self.layout,
                            CppLayoutPolicy::StackAllocated { size: 1, align: 1 }
                        );
                        assert!(is_copy);
                        writeln!(
                            state,
                            r#"
public:
    operator bool() {{
        return data[0];
    }}
    Bool(bool b) {{
        data[0] = b;
    }}
private:
    "#,
                        )?;
                    }
                    if !is_copy {
                        writeln!(state, "   bool drop_flag;")?;
                    }
                    let (alloc_heap, free_heap, copy_data) = match &self.layout {
                        CppLayoutPolicy::StackAllocated { .. } => (
                            "".to_owned(),
                            "".to_owned(),
                            "this->data = other.data;".to_owned(),
                        ),
                        CppLayoutPolicy::HeapAllocated {
                            size_fn,
                            alloc_fn,
                            free_fn,
                        } => (
                            format!("data = {alloc_fn}();"),
                            format!("{free_fn}(data);"),
                            format!("memcpy(this->data, other.data, {size_fn}());"),
                        ),
                        CppLayoutPolicy::OnlyByRef => unreachable!(),
                    };
                    writeln!(state, "public:")?;
                    if is_copy {
                        writeln!(
                            state,
                            r#"
    {ty}() {{ {alloc_heap} }}
    ~{ty}() {{ {free_heap} }}
    {ty}(const {ty}& other) {{
        {alloc_heap}
        {copy_data}
    }}
    {ty}& operator=(const {ty}& other) {{
        {copy_data}
        return *this;
    }}
    {ty}({ty}&& other) {{
        {alloc_heap}
        {copy_data}
    }}
    {ty}& operator=({ty}&& other) {{
        {copy_data}
        return *this;
    }}
    "#,
                            ty = self.ty.path.name(),
                        )?;
                    } else {
                        let drop_in_place = self
                            .wellknown_traits
                            .iter()
                            .find_map(|x| match x {
                                ZngurWellknownTraitData::Drop { drop_in_place } => {
                                    Some(drop_in_place)
                                }
                                _ => None,
                            })
                            .unwrap();
                        writeln!(
                            state,
                            r#"
    {ty}() : drop_flag(false) {{ {alloc_heap} }}
    ~{ty}() {{
        if (drop_flag) {{
            {drop_in_place}(&data[0]);
        }}
        {free_heap}
    }}
    {ty}(const {ty}& other) = delete;
    {ty}& operator=(const {ty}& other) = delete;
    {ty}({ty}&& other) : drop_flag(false) {{
        {alloc_heap}
        *this = ::std::move(other);
    }}
    {ty}& operator=({ty}&& other) {{
        if (this != &other)
        {{
            if (drop_flag) {{
                {drop_in_place}(&data[0]);
            }}
            this->drop_flag = other.drop_flag;
            {copy_data}
            other.drop_flag = false;
        }}
        return *this;
    }}
    "#,
                            ty = self.ty.path.name(),
                        )?;
                    }
                    match &self.from_trait {
                        Some(RustTrait::Fn { inputs, output, .. }) => {
                            let as_std_function = format!(
                                "::std::function< {}({})>",
                                output.into_cpp(),
                                inputs.iter().map(|x| x.into_cpp()).join(", ")
                            );
                            writeln!(
                                state,
                                r#"
    static inline {ty} make_box({as_std_function} f);
    "#,
                                ty = self.ty.path.name(),
                            )?;
                        }
                        Some(RustTrait::Normal { .. }) => {
                            writeln!(
                                state,
                                r#"
                        template<typename T, typename... Args>
                        static {ty} make_box(Args&&... args);
                        "#,
                                ty = self.ty.path.name(),
                            )?;
                        }
                        None => (),
                    }
                }
            }
            if let Some((rust_link_name, cpp_ty)) = &self.cpp_value {
                writeln!(
                    state,
                    r#"
                    inline {cpp_ty}& cpp() {{
                        return (*{rust_link_name}(&data[0])).as_cpp< {cpp_ty} >();
                    }}"#
                )?;
            }
            for method in &self.methods {
                write!(state, "static ")?;
                method.sig.emit_cpp_header(state, &method.name)?;
                if method.kind != ZngurMethodReceiver::Static {
                    let CppFnSig {
                        rust_link_name: _,
                        inputs,
                        output,
                    } = &method.sig;
                    writeln!(
                        state,
                        "{output} {fn_name}({dyn_api_def}{input_defs}) {const_kw} noexcept ;",
                        fn_name = &method.name,
                        dyn_api_def = if inputs.len().saturating_sub(1) == 0 {
                            "__zngur_dyn_api* api"
                        } else {
                            "__zngur_dyn_api* api, "
                        },
                        input_defs = inputs
                            .iter()
                            .skip(1)
                            .enumerate()
                            .map(|(n, ty)| format!("{ty} i{n}"))
                            .join(", "),
                        const_kw = if method.kind != ZngurMethodReceiver::Ref(Mutability::Not) {
                            ""
                        } else {
                            "const"
                        },
                    )?;
                }
            }
            for constructor in &self.constructors {
                writeln!(
                    state,
                    "{fn_name}({input_defs}) noexcept ;",
                    fn_name = &self.ty.path.0.last().unwrap(),
                    input_defs = constructor
                        .inputs
                        .iter()
                        .enumerate()
                        .map(|(n, ty)| format!("{ty} i{n}"))
                        .join(", "),
                )?;
            }
            for field in &self.fields {
                writeln!(
                    state,
                    "[[no_unique_address]] ::rust::FieldOwned<{}, {}> {};",
                    field.ty.into_cpp(),
                    field.offset,
                    cpp_handle_field_name(&field.name),
                )?;
            }
            writeln!(state, "}};")
        })?;
        let ty = &self.ty;
        if self.layout != CppLayoutPolicy::OnlyByRef {
            match &self.layout {
                CppLayoutPolicy::StackAllocated { size, align: _ } => {
                    writeln!(
                        state,
                        r#"
namespace rust {{
    template<>
    inline size_t __zngur_internal_size_of< {ty} >() noexcept {{
        return {size};
    }}
        "#,
                    )?;
                }
                CppLayoutPolicy::HeapAllocated { size_fn, .. } => {
                    writeln!(
                        state,
                        r#"
namespace rust {{
    template<>
    inline size_t __zngur_internal_size_of< {ty} >() noexcept {{
        return {size_fn}();
    }}
        "#,
                    )?;
                }
                CppLayoutPolicy::OnlyByRef => unreachable!(),
            }

            if is_copy {
                writeln!(
                    state,
                    r#"
        template<>
        inline void __zngur_internal_check_init< {ty} >(const {ty}&) noexcept {{
        }}

        template<>
        inline void __zngur_internal_assume_init< {ty} >({ty}&) noexcept {{
        }}
    
        template<>
        inline void __zngur_internal_assume_deinit< {ty} >({ty}&) noexcept {{
        }}
"#,
                )?;
            } else {
                writeln!(
                    state,
                    r#"
        template<>
        inline void __zngur_internal_check_init< {ty} >(const {ty}& t) noexcept {{
            if (!t.drop_flag) {{
                ::std::cerr << "Use of uninitialized or moved Zngur Rust object with type {ty}" << ::std::endl;
                while (true) raise(SIGSEGV);
            }}
        }}

        template<>
        inline void __zngur_internal_assume_init< {ty} >({ty}& t) noexcept {{
            t.drop_flag = true;
        }}
    
        template<>
        inline void __zngur_internal_assume_deinit< {ty} >({ty}& t) noexcept {{
            ::rust::__zngur_internal_check_init< {ty} >(t);
            t.drop_flag = false;
        }}
"#,
                )?;
            }
            writeln!(
                state,
                r#"
    template<>
    inline uint8_t* __zngur_internal_data_ptr< {ty} >({ty} const & t) noexcept {{
        return const_cast<uint8_t*>(&t.data[0]);
    }}
}}
"#,
            )?;
        }
        self.emit_ref_specialization(state)?;
        self.emit_field_specialization(state)?;
        Ok(())
    }

    fn emit_cpp_fn_defs(
        &self,
        state: &mut State,
        traits: &HashMap<RustTrait, CppTraitDefinition>,
    ) -> std::fmt::Result {
        let is_unsized = self
            .wellknown_traits
            .contains(&ZngurWellknownTraitData::Unsized);
        let cpp_type = &self.ty.to_string();
        let my_name = cpp_type.strip_prefix("::").unwrap();
        for c in &self.constructors {
            let fn_name = my_name.to_owned() + "::" + self.ty.path.0.last().unwrap();
            let CppFnSig {
                inputs,
                output: _,
                rust_link_name,
            } = c;
            writeln!(
                state,
                "inline {fn_name}({dyn_api_def}{input_defs}) noexcept {{
            ::rust::__zngur_internal_assume_init(*this);
            {rust_link_name}({input_args}::rust::__zngur_internal_data_ptr(*this));
            {deinits}
        }}",
                dyn_api_def = if inputs.len().saturating_sub(1) == 0 {
                    "__zngur_dyn_api* api"
                } else {
                    "__zngur_dyn_api* api, "
                },
                input_defs = inputs
                    .iter()
                    .enumerate()
                    .map(|(n, ty)| format!("{ty} i{n}"))
                    .join(", "),
                input_args = (0..inputs.len())
                    .map(|n| format!("::rust::__zngur_internal_data_ptr(i{n}), "))
                    .join(""),
                deinits = (0..inputs.len())
                    .map(|n| format!("::rust::__zngur_internal_assume_deinit(i{n});"))
                    .join("\n"),
            )?;
        }
        match self.from_trait.as_ref().and_then(|k| traits.get(k)) {
            Some(CppTraitDefinition::Fn { sig }) => {
                let as_std_function = format!(
                    "::std::function< {}({})>",
                    sig.output,
                    sig.inputs.iter().join(", ")
                );
                let ii_names = sig
                    .inputs
                    .iter()
                    .enumerate()
                    .map(|(n, x)| format!("::rust::__zngur_internal_move_from_rust< {x} >(i{n})"))
                    .join(", ");
                let uint8_t_ix = sig
                    .inputs
                    .iter()
                    .enumerate()
                    .map(|(n, _)| format!("uint8_t* i{n},"))
                    .join(" ");
                let out_ty = &sig.output;
                writeln!(
                    state,
                    r#"
{my_name} {my_name}::make_box({as_std_function} f) {{
auto data = new {as_std_function}(f);
{my_name} o;
::rust::__zngur_internal_assume_init(o);
{link_name}(
reinterpret_cast<uint8_t*>(data),
[](uint8_t *d) {{ delete reinterpret_cast< {as_std_function}*>(d); }},
[](uint8_t *d, {uint8_t_ix} uint8_t *o) {{
auto dd = reinterpret_cast< {as_std_function} *>(d);
{out_ty} oo = (*dd)({ii_names});
::rust::__zngur_internal_move_to_rust< {out_ty} >(o, oo);
}},
::rust::__zngur_internal_data_ptr(o));
return o;
}}
"#,
                    link_name = sig.rust_link_name,
                )?;
            }
            Some(CppTraitDefinition::Normal {
                as_ty,
                methods: _,
                link_name,
                link_name_ref: _,
            }) => {
                writeln!(
                    state,
                    r#"
template<typename T, typename... Args>
{my_name} {my_name}::make_box(Args&&... args) {{
auto data = new T(::std::forward<Args>(args)...);
auto data_as_impl = dynamic_cast< {as_ty}*>(data);
{my_name} o;
::rust::__zngur_internal_assume_init(o);
{link_name}(
reinterpret_cast<uint8_t*>(data_as_impl),
[](uint8_t *d) {{ delete reinterpret_cast< {as_ty} *>(d); }},
"#,
                )?;
                writeln!(
                    state,
                    r#"
::rust::__zngur_internal_data_ptr(o));
return o;
}}
"#,
                )?;
            }
            None => (),
        }
        match self.from_trait_ref.as_ref().and_then(|k| traits.get(k)) {
            Some(CppTraitDefinition::Fn { .. }) => {
                todo!()
            }
            Some(CppTraitDefinition::Normal {
                as_ty,
                methods: _,
                link_name: _,
                link_name_ref,
            }) => {
                for ref_kind in ["Ref", " RefMut"] {
                    writeln!(
                        state,
                        r#"
rust::{ref_kind}< {my_name} >::{ref_kind}({as_ty}& args) {{
auto data_as_impl = &args;
::rust::__zngur_internal_assume_init(*this);
{link_name_ref}(
(uint8_t *)data_as_impl,
"#,
                    )?;
                    writeln!(
                        state,
                        r#"
::rust::__zngur_internal_data_ptr(*this));
}}
"#,
                    )?;
                }
            }
            None => (),
        }
        for method in &self.methods {
            let fn_name = my_name.to_owned() + "::" + &method.name;
            method.sig.emit_cpp_def(state, &fn_name)?;
            if let ZngurMethodReceiver::Ref(m) = method.kind {
                let ref_kinds: &[&str] = match m {
                    Mutability::Mut => &["RefMut"],
                    Mutability::Not => &["Ref", "RefMut"],
                };
                let field_kinds: &[&str] = match m {
                    Mutability::Mut => &["FieldOwned", "FieldRefMut"],
                    Mutability::Not => &["FieldOwned", "FieldRefMut", "FieldRef"],
                };
                for field_kind in field_kinds {
                    let CppFnSig {
                        rust_link_name: _,
                        inputs,
                        output,
                    } = &method.sig;
                    writeln!(
                        state,
                        "template<size_t OFFSET>
                        inline {output} rust::{field_kind}< {ty}, OFFSET >::{method_name}({input_defs}) const noexcept {{
                    return {fn_name}(*this{input_args});
                }}",
                        ty = &self.ty,
                        method_name = &method.name,
                        input_defs = inputs
                            .iter()
                            .skip(1)
                            .enumerate()
                            .map(|(n, ty)| format!("{ty} i{n}"))
                            .join(", "),
                        input_args = (0..inputs.len() - 1)
                            .map(|n| format!(", ::std::move(i{n})"))
                            .join("")
                    )?;
                }
                for ref_kind in ref_kinds {
                    let CppFnSig {
                        rust_link_name: _,
                        inputs,
                        output,
                    } = &method.sig;
                    writeln!(
                        state,
                        "inline {output} rust::{ref_kind}< {ty} >::{method_name}({dyn_api_def}{input_defs}) const noexcept {{
                    return {fn_name}(api, *this{input_args});
                }}",
                        ty = &self.ty,
                        method_name = &method.name,
                        dyn_api_def = if inputs.len().saturating_sub(1) == 0 {
                            "__zngur_dyn_api* api"
                        } else {
                            "__zngur_dyn_api* api, "
                        },
                        input_defs = inputs
                            .iter()
                            .skip(1)
                            .enumerate()
                            .map(|(n, ty)| format!("{ty} i{n}"))
                            .join(", "),
                        input_args = (0..inputs.len() - 1)
                            .map(|n| format!(", ::std::move(i{n})"))
                            .join("")
                    )?;
                }
            }
            if !is_unsized
                && !matches!(self.layout, CppLayoutPolicy::OnlyByRef)
                && method.kind != ZngurMethodReceiver::Static
            {
                let CppFnSig {
                    rust_link_name: _,
                    inputs,
                    output,
                } = &method.sig;
                writeln!(
                    state,
                    "inline {output} {fn_name}({dyn_api_def}{input_defs}) {const_kw} noexcept {{
                    return {fn_name}(api, {this_arg}{input_args});
                }}",
                    this_arg = match method.kind {
                        ZngurMethodReceiver::Ref(_) => "*this",
                        ZngurMethodReceiver::Move => "::std::move(*this)",
                        ZngurMethodReceiver::Static => unreachable!(),
                    },
                    dyn_api_def = if inputs.len().saturating_sub(1) == 0 {
                        "__zngur_dyn_api* api"
                    } else {
                        "__zngur_dyn_api* api, "
                    },
                    input_defs = inputs
                        .iter()
                        .skip(1)
                        .enumerate()
                        .map(|(n, ty)| format!("{ty} i{n}"))
                        .join(", "),
                    input_args = (0..inputs.len() - 1)
                        .map(|n| format!(", ::std::move(i{n})"))
                        .join(""),
                    const_kw = if method.kind != ZngurMethodReceiver::Ref(Mutability::Not) {
                        ""
                    } else {
                        "const"
                    },
                )?;
            }
        }
        let is_unsized = self
            .wellknown_traits
            .contains(&ZngurWellknownTraitData::Unsized);
        for tr in &self.wellknown_traits {
            match tr {
                ZngurWellknownTraitData::Debug {
                    pretty_print,
                    debug_print: _, // TODO: use it
                } => {
                    if !is_unsized {
                        writeln!(
                            state,
                            r#"
            namespace rust {{
                template<>
                struct ZngurPrettyPrinter< {ty} > {{
                    static inline void print({ty} const& t) {{
                        ::rust::__zngur_internal_check_init< {ty} >(t);
                        {pretty_print}(&t.data[0]);
                    }}
                }};

                template<>
                struct ZngurPrettyPrinter< Ref< {ty} > > {{
                    static inline void print(Ref< {ty} > const& t) {{
                        ::rust::__zngur_internal_check_init< Ref< {ty} > >(t);
                        {pretty_print}(reinterpret_cast<uint8_t*>(t.data));
                    }}
                }};

                template<>
                struct ZngurPrettyPrinter< RefMut< {ty} > > {{
                    static inline void print(RefMut< {ty} > const& t) {{
                        ::rust::__zngur_internal_check_init< RefMut< {ty} > >(t);
                        {pretty_print}(reinterpret_cast<uint8_t*>(t.data));
                    }}
                }};

                template<size_t OFFSET>
                struct ZngurPrettyPrinter< FieldOwned< {ty}, OFFSET > > {{
                    static inline void print(FieldOwned< {ty}, OFFSET > const& t) {{
                        ZngurPrettyPrinter< Ref< {ty} > >::print(t);
                    }}
                }};

                template<size_t OFFSET>
                struct ZngurPrettyPrinter< FieldRef< {ty}, OFFSET > > {{
                    static inline void print(FieldRef< {ty}, OFFSET > const& t) {{
                        ZngurPrettyPrinter< Ref< {ty} > >::print(t);
                    }}
                }};

                template<size_t OFFSET>
                struct ZngurPrettyPrinter< FieldRefMut< {ty}, OFFSET > > {{
                    static inline void print(FieldRefMut< {ty}, OFFSET > const& t) {{
                        ZngurPrettyPrinter< Ref< {ty} > >::print(t);
                    }}
                }};
            }}"#,
                            ty = self.ty,
                        )?;
                    } else {
                        writeln!(
                            state,
                            r#"
            namespace rust {{
                template<>
                struct ZngurPrettyPrinter< Ref< {ty} > > {{
                    static inline void print(Ref< {ty} > const& t) {{
                        ::rust::__zngur_internal_check_init< Ref< {ty} > >(t);
                        {pretty_print}(::rust::__zngur_internal_data_ptr< Ref< {ty} > >(t));
                    }}
                }};

                template<>
                struct ZngurPrettyPrinter< RefMut< {ty} > > {{
                    static inline void print(RefMut< {ty} > const& t) {{
                        ::rust::__zngur_internal_check_init< RefMut< {ty} > >(t);
                        {pretty_print}(::rust::__zngur_internal_data_ptr< RefMut< {ty} > >(t));
                    }}
                }};
            }}"#,
                            ty = self.ty,
                        )?;
                    }
                }
                ZngurWellknownTraitData::Unsized
                | ZngurWellknownTraitData::Copy
                | ZngurWellknownTraitData::Drop { .. } => {}
            }
        }
        Ok(())
    }

    fn get_link_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for method in &self.methods {
            names.push(&method.sig.rust_link_name);
        }
        for c in &self.constructors {
            names.push(&c.rust_link_name);
        }
        assert!(
            self.cpp_value.is_none(),
            "C++ value not implemented for dyn api"
        );
        if let CppLayoutPolicy::HeapAllocated {
            size_fn,
            alloc_fn,
            free_fn,
        } = &self.layout
        {
            names.push(size_fn);
            names.push(alloc_fn);
            names.push(free_fn);
        }
        for tr in &self.wellknown_traits {
            match tr {
                ZngurWellknownTraitData::Debug {
                    pretty_print,
                    debug_print,
                } => {
                    names.push(pretty_print);
                    names.push(debug_print);
                }
                ZngurWellknownTraitData::Unsized | ZngurWellknownTraitData::Copy => (),
                ZngurWellknownTraitData::Drop { drop_in_place } => {
                    names.push(drop_in_place);
                }
            }
        }
        return names;
    }

    fn emit_rust_links(&self, state: &mut State) -> std::fmt::Result {
        for method in &self.methods {
            method.sig.emit_rust_link_decl(state)?;
        }
        for c in &self.constructors {
            c.emit_rust_link_decl(state)?;
        }
        if let Some(cpp_value) = &self.cpp_value {
            writeln!(
                state,
                "::rust::ZngurCppOpaqueOwnedObject* {}(uint8_t*);",
                cpp_value.0
            )?;
        }
        if let CppLayoutPolicy::HeapAllocated {
            size_fn,
            alloc_fn,
            free_fn,
        } = &self.layout
        {
            writeln!(state, "size_t {size_fn}();")?;
            writeln!(state, "uint8_t* {alloc_fn}();")?;
            writeln!(state, "void {free_fn}(uint8_t*);")?;
        }
        for tr in &self.wellknown_traits {
            match tr {
                ZngurWellknownTraitData::Debug {
                    pretty_print,
                    debug_print,
                } => {
                    writeln!(state, "void {pretty_print}(uint8_t *data);")?;
                    writeln!(state, "void {debug_print}(uint8_t *data);")?;
                }
                ZngurWellknownTraitData::Unsized | ZngurWellknownTraitData::Copy => (),
                ZngurWellknownTraitData::Drop { drop_in_place } => {
                    writeln!(state, "void {drop_in_place}(uint8_t *data);")?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct CppFile {
    pub type_defs: Vec<CppTypeDefinition>,
    pub trait_defs: HashMap<RustTrait, CppTraitDefinition>,
    pub fn_defs: Vec<CppFnDefinition>,
    pub exported_fn_defs: Vec<CppExportedFnDefinition>,
    pub exported_impls: Vec<CppExportedImplDefinition>,
    pub additional_includes: String,
    pub panic_to_exception: bool,
}

impl CppFile {
    fn emit_h_file(&self, state: &mut State) -> std::fmt::Result {
        state.text += r#"
#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <csignal>
#include <array>
#include <iostream>
#include <functional>
#include <math.h>
#include <dlfcn.h>
"#;
        state.text += &self.additional_includes;
        if self.panic_to_exception {
            state.text += indoc::indoc! { r#"
            namespace rust {
                class Panic {};
            }
            extern "C" {
                uint8_t __zngur_detect_panic();
                void __zngur_take_panic();
            }
            "#};
        }
        state.text += r#"
#define zngur_dbg(x) (::rust::zngur_dbg_impl(__FILE__, __LINE__, #x, x))

namespace rust {
    template<typename T>
    uint8_t* __zngur_internal_data_ptr(const T& t) noexcept ;

    template<typename T>
    void __zngur_internal_assume_init(T& t) noexcept ;

    template<typename T>
    void __zngur_internal_assume_deinit(T& t) noexcept ;

    template<typename T>
    inline size_t __zngur_internal_size_of() noexcept ;

    template<typename T>
    inline void __zngur_internal_move_to_rust(uint8_t* dst, T& t) noexcept {
        memcpy(dst, ::rust::__zngur_internal_data_ptr(t), ::rust::__zngur_internal_size_of<T>());
        ::rust::__zngur_internal_assume_deinit(t);
    }

    template<typename T>
    inline T __zngur_internal_move_from_rust(uint8_t* src) noexcept {
        T t;
        ::rust::__zngur_internal_assume_init(t);
        memcpy(::rust::__zngur_internal_data_ptr(t), src, ::rust::__zngur_internal_size_of<T>());
        return t;
    }

    template<typename T>
    inline void __zngur_internal_check_init(const T&) noexcept {
    }

    class ZngurCppOpaqueOwnedObject {
        uint8_t* data;
        void (*destructor)(uint8_t*);

    public:
        template<typename T, typename... Args>
        inline static ZngurCppOpaqueOwnedObject build(Args&&... args) {
            ZngurCppOpaqueOwnedObject o;
            o.data = reinterpret_cast<uint8_t*>(new T(::std::forward<Args>(args)...));
            o.destructor = [](uint8_t* d) {
                delete reinterpret_cast<T*>(d);
            };
            return o;
        }

        template<typename T>
        inline T& as_cpp() {
            return *reinterpret_cast<T *>(data);
        }
    };

    template<typename T>
    struct Ref;

    template<typename T>
    struct RefMut;

    template<typename T, size_t OFFSET>
    struct FieldOwned;

    template<typename T, size_t OFFSET>
    struct FieldRef;

    template<typename T, size_t OFFSET>
    struct FieldRefMut;

    template<typename... T>
    struct Tuple;

    using Unit = Tuple<>;

    template<typename T>
    struct ZngurPrettyPrinter;

    class Inherent;

    template<typename Type, typename Trait = Inherent>
    class Impl;

    template<typename T>
    T&& zngur_dbg_impl(const char* file_name, int line_number, const char* exp, T&& input) {
        ::std::cerr << "[" << file_name << ":" << line_number << "] " << exp << " = ";
        ZngurPrettyPrinter<typename ::std::remove_reference<T>::type>::print(input);
        return ::std::forward<T>(input);
    }
"#;
        for ty in [8, 16, 32, 64]
            .into_iter()
            .flat_map(|x| [format!("int{x}_t"), format!("uint{x}_t")])
            .chain([8, 16, 32, 64].into_iter().flat_map(|x| {
                [
                    format!("::rust::Ref<int{x}_t>"),
                    format!("::rust::Ref<uint{x}_t>"),
                    format!("::rust::RefMut<int{x}_t>"),
                    format!("::rust::RefMut<uint{x}_t>"),
                ]
            }))
            .chain([
                "::rust::ZngurCppOpaqueOwnedObject".to_string(),
                "::double_t".to_string(),
                "::float_t".to_string(),
                "::size_t".to_string(),
            ])
        {
            if ty == "::size_t" {
                writeln!(state, "#ifdef __APPLE__")?;
            }
            writeln!(
                state,
                r#"
    template<>
    inline uint8_t* __zngur_internal_data_ptr< {ty} >(const {ty}& t) noexcept {{
        return const_cast<uint8_t*>(reinterpret_cast<const uint8_t*>(&t));
    }}

    template<>
    inline void __zngur_internal_assume_init< {ty} >({ty}&) noexcept {{}}
    template<>
    inline void __zngur_internal_assume_deinit< {ty} >({ty}&) noexcept {{}}

    template<>
    inline size_t __zngur_internal_size_of< {ty} >() noexcept {{
        return sizeof({ty});
    }}

    template<>
    inline uint8_t* __zngur_internal_data_ptr< {ty}*>({ty}* const & t) noexcept {{
        return const_cast<uint8_t*>(reinterpret_cast<const uint8_t*>(&t));
    }}

    template<>
    inline void __zngur_internal_assume_init< {ty}*>({ty}*&) noexcept {{}}
    template<>
    inline void __zngur_internal_assume_deinit< {ty}*>({ty}*&) noexcept {{}}

    template<>
    inline uint8_t* __zngur_internal_data_ptr< {ty} const*>({ty} const* const & t) noexcept {{
        return const_cast<uint8_t*>(reinterpret_cast<const uint8_t*>(&t));
    }}

    template<>
    inline void __zngur_internal_assume_init< {ty} const*>({ty} const*&) noexcept {{}}
    template<>
    inline void __zngur_internal_assume_deinit< {ty} const*>({ty} const*&) noexcept {{}}

    template<>
    struct Ref< {ty} > {{
        Ref() {{
            data = 0;
        }}
        Ref(const {ty}& t) {{
            data = reinterpret_cast<size_t>(__zngur_internal_data_ptr(t));
        }}

        {ty}& operator*() {{
            return *reinterpret_cast< {ty}*>(data);
        }}
        private:
            size_t data;
        friend uint8_t* ::rust::__zngur_internal_data_ptr<Ref< {ty} > >(const ::rust::Ref< {ty} >& t) noexcept ;
        friend ::rust::ZngurPrettyPrinter< Ref< {ty} > >;
    }};

    template<>
    struct RefMut< {ty} > {{
        RefMut() {{
            data = 0;
        }}
        RefMut({ty}& t) {{
            data = reinterpret_cast<size_t>(__zngur_internal_data_ptr(t));
        }}

        {ty}& operator*() {{
            return *reinterpret_cast< {ty}*>(data);
        }}
        private:
            size_t data;
        friend uint8_t* ::rust::__zngur_internal_data_ptr<RefMut< {ty} > >(const ::rust::RefMut< {ty} >& t) noexcept ;
        friend ::rust::ZngurPrettyPrinter< Ref< {ty} > >;
    }};
"#
            )?;
            if ty.starts_with("int")
                || ty.starts_with("uint")
                || ty.starts_with("::size_t")
                || ty.starts_with("::double")
                || ty.starts_with("::float")
            {
                writeln!(
                    state,
                    r#"
    template<>
    struct ZngurPrettyPrinter<{ty}> {{
        static inline void print({ty} const& t) {{
            ::std::cerr << t << ::std::endl;
        }}
    }};
                    "#
                )?;
            }
            if ty == "::size_t" {
                writeln!(state, "#endif")?;
            }
        }
        writeln!(state, "}}")?;
        writeln!(state, "extern \"C\" {{")?;
        for f in &self.fn_defs {
            f.sig.emit_rust_link_decl(state)?;
        }
        for td in &self.type_defs {
            td.emit_rust_links(state)?;
        }
        for (_, td) in &self.trait_defs {
            td.emit_rust_links(state)?;
        }
        writeln!(state, "}}")?;

        writeln!(state, "class __zngur_dyn_api {{")?;
        writeln!(state, "public:")?;
        // constructor
        writeln!(
            state,
            "  explicit __zngur_dyn_api(void *dylib_handle) : _dylib_handle(dylib_handle) {{}}"
        )?;

        // fields
        writeln!(state, "  void * _dylib_handle;")?;
        for f in &self.fn_defs {
            write!(state, "  ")?;
            f.sig.emit_zngur_dyn_api_field(state)?;
            writeln!(state)?;
        }
        for td in &self.type_defs {
            for n in td.get_link_names() {
                writeln!(
                    state,
                    r###"  decltype(&::{link_name}) {link_name};"###,
                    link_name = n,
                )?;
            }
        }

        writeln!(state, "\n")?;

        // functions
        writeln!(state, "  bool init(std::string &error_message) {{")?;
        let write_link_name = |state: &mut State, link_name: &str| {
            writeln!(
                state,
                indoc! { r###"
                    {{
                    void* dlsym_handle = dlsym(_dylib_handle, "{link_name}");
                    if (dlsym_handle == nullptr) {{
                        error_message = "Failed to load {link_name} from dynamic library: " + std::string(dlerror());
                        return false;
                    }}
                    {link_name} = reinterpret_cast<decltype(&::{link_name})>(dlsym_handle);
                    }}
                "###},
                link_name = link_name,
            )
        };
        for f in &self.fn_defs {
            write_link_name(state, &f.sig.rust_link_name)?;
        }
        for td in &self.type_defs {
            for n in td.get_link_names() {
                write_link_name(state, n)?;
            }
        }
        writeln!(state, "  return true;")?;
        writeln!(state, "  }}")?;

        if !self.trait_defs.is_empty() {
            panic!("zngur dyn api has not implemented for trait_defs yet.");
        }
        writeln!(state, "\n")?;

        writeln!(state, "}};")?;

        for td in &self.type_defs {
            td.ty.emit_header(state)?;
        }
        for imp in &self.exported_impls {
            imp.ty.emit_header(state)?;
            if let Some(tr) = &imp.tr {
                tr.emit_header(state)?;
            }
        }
        for (_, td) in &self.trait_defs {
            td.emit(state)?;
        }
        for td in &self.type_defs {
            td.emit(state)?;
        }
        for td in &self.type_defs {
            td.emit_cpp_fn_defs(state, &self.trait_defs)?;
        }
        for fd in &self.fn_defs {
            fd.emit_cpp_def(state)?;
        }
        for func in &self.exported_fn_defs {
            writeln!(state, "namespace rust {{ namespace exported_functions {{")?;
            write!(state, "   {} {}(", func.sig.output, func.name)?;
            for (n, ty) in func.sig.inputs.iter().enumerate() {
                if n != 0 {
                    write!(state, ", ")?;
                }
                write!(state, "{ty} i{n}")?;
            }
            writeln!(state, ");")?;
            writeln!(state, "}} }}")?;
        }
        for imp in &self.exported_impls {
            writeln!(
                state,
                "namespace rust {{ template<> class Impl< {}, {} > {{ public:",
                imp.ty,
                match &imp.tr {
                    Some(x) => format!("{x}"),
                    None => "::rust::Inherent".to_string(),
                }
            )?;
            for (name, sig) in &imp.methods {
                write!(state, "   static {} {}(", sig.output, name)?;
                for (n, ty) in sig.inputs.iter().enumerate() {
                    if n != 0 {
                        write!(state, ", ")?;
                    }
                    write!(state, "{ty} i{n}")?;
                }
                writeln!(state, ");")?;
            }
            writeln!(state, "}}; }}")?;
        }
        Ok(())
    }

    fn emit_cpp_file(&self, state: &mut State, is_really_needed: &mut bool) -> std::fmt::Result {
        writeln!(state, r#"#include "./generated.h""#)?;
        writeln!(state, "extern \"C\" {{")?;
        for t in &self.trait_defs {
            *is_really_needed = true;
            t.1.emit_cpp(state)?;
        }
        for func in &self.exported_fn_defs {
            *is_really_needed = true;
            func.sig.emit_rust_link(state)?;
            writeln!(state, "{{")?;
            writeln!(
                state,
                "   {} oo = ::rust::exported_functions::{}({});",
                func.sig.output,
                func.name,
                func.sig
                    .inputs
                    .iter()
                    .enumerate()
                    .map(|(n, ty)| {
                        format!("::rust::__zngur_internal_move_from_rust< {ty} >(i{n})")
                    })
                    .join(", "),
            )?;
            writeln!(state, "   ::rust::__zngur_internal_move_to_rust(o, oo);")?;
            writeln!(state, "}}")?;
        }
        for imp in &self.exported_impls {
            *is_really_needed = true;
            for (name, sig) in &imp.methods {
                sig.emit_rust_link(state)?;
                writeln!(state, "{{")?;
                writeln!(
                    state,
                    "   {} oo = ::rust::Impl< {}, {} >::{}({});",
                    sig.output,
                    imp.ty,
                    match &imp.tr {
                        Some(x) => format!("{x}"),
                        None => "::rust::Inherent".to_string(),
                    },
                    name,
                    sig.inputs
                        .iter()
                        .enumerate()
                        .map(|(n, ty)| {
                            format!("::rust::__zngur_internal_move_from_rust< {ty} >(i{n})")
                        })
                        .join(", "),
                )?;
                writeln!(state, "   ::rust::__zngur_internal_move_to_rust(o, oo);")?;
                writeln!(state, "}}")?;
            }
        }
        writeln!(state, "}}")?;
        Ok(())
    }

    pub fn render(self) -> (String, Option<String>) {
        let mut h_file = State {
            text: "".to_owned(),
            panic_to_exception: self.panic_to_exception,
        };
        let mut cpp_file = State {
            text: "".to_owned(),
            panic_to_exception: self.panic_to_exception,
        };
        self.emit_h_file(&mut h_file).unwrap();
        let mut is_cpp_needed = false;
        self.emit_cpp_file(&mut cpp_file, &mut is_cpp_needed)
            .unwrap();
        h_file.remove_no_except_in_panic();
        (h_file.text, is_cpp_needed.then_some(cpp_file.text))
    }
}

pub fn cpp_handle_keyword(name: &str) -> &str {
    match name {
        "new" => "new_",
        "default" => "default_",
        x => x,
    }
}

pub fn cpp_handle_field_name(name: &str) -> String {
    if name.parse::<u32>().is_ok() {
        return format!("f{name}");
    }
    cpp_handle_keyword(name).to_owned()
}
