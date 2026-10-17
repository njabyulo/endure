//! Parse source, reject unsupported syntax, and erase TypeScript into ES modules.
//! SWC prepares code; V8 still compiles and executes the resulting JavaScript.

use super::modules::ModuleError;
use swc_core::{
    common::{sync::Lrc, FileName, Globals, Mark, SourceMap, Span, Spanned, GLOBALS},
    ecma::{
        ast::*,
        codegen::to_code_default,
        parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax, TsSyntax},
        transforms::{
            base::{fixer::fixer, hygiene::hygiene, resolver},
            typescript::{typescript, Config},
        },
        visit::{Visit, VisitWith},
    },
};

pub(crate) struct Transpiled {
    pub(crate) javascript: String,
    pub(crate) imports: Vec<String>,
}

pub(crate) fn transpile(
    filename: &str,
    source: &str,
    is_typescript: bool,
) -> Result<Transpiled, ModuleError> {
    let cm: Lrc<SourceMap> = Default::default();
    let file = cm.new_source_file(FileName::Custom(filename.into()).into(), source.to_owned());
    let syntax = if is_typescript {
        Syntax::Typescript(TsSyntax::default())
    } else {
        Syntax::Es(EsSyntax::default())
    };
    let lexer = Lexer::new(syntax, EsVersion::EsNext, StringInput::from(&*file), None);
    let mut parser = Parser::new_from(lexer);
    let diagnostic = |span: Span, message: String| {
        let loc = cm.lookup_char_pos(span.lo);
        ModuleError::new(
            filename,
            format!(
                "SyntaxError: {message} ({}:{})",
                loc.line,
                loc.col_display + 1
            ),
        )
    };
    let module = parser
        .parse_module()
        .map_err(|error| diagnostic(error.span(), error.kind().msg().into_owned()))?;
    if let Some(error) = parser.take_errors().first() {
        return Err(diagnostic(error.span(), error.kind().msg().into_owned()));
    }
    let mut unsupported = Unsupported::default();
    module.visit_with(&mut unsupported);
    if let Some((span, message)) = unsupported.error {
        return Err(diagnostic(span, message.into()));
    }
    let program = GLOBALS.set(&Globals::new(), || {
        let mut program = Program::Module(module);
        if is_typescript {
            let unresolved = Mark::new();
            let top_level = Mark::new();
            program = program.apply(resolver(unresolved, top_level, true));
            program = program.apply(typescript(
                Config {
                    verbatim_module_syntax: true,
                    ..Default::default()
                },
                unresolved,
                top_level,
            ));
            program = program.apply(hygiene());
            program = program.apply(fixer(None));
        }
        program
    });
    let mut imports = Vec::new();
    if let Program::Module(module) = &program {
        for item in &module.body {
            let source = match item {
                ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => Some(&import.src),
                ModuleItem::ModuleDecl(ModuleDecl::ExportAll(export)) => Some(&export.src),
                ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) => export.src.as_ref(),
                _ => None,
            };
            if let Some(source) = source {
                let specifier = source.value.as_str().ok_or_else(|| {
                    diagnostic(source.span, "module specifier must be valid Unicode".into())
                })?;
                imports.push(specifier.to_owned());
            }
        }
    }
    Ok(Transpiled {
        javascript: to_code_default(cm, None, &program),
        imports,
    })
}

#[derive(Default)]
struct Unsupported {
    error: Option<(Span, &'static str)>,
}
impl Visit for Unsupported {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if matches!(call.callee, Callee::Import(_)) {
            self.error
                .get_or_insert((call.span, "dynamic imports are not supported"));
        }
        call.visit_children_with(self);
    }
    fn visit_decorator(&mut self, decorator: &Decorator) {
        self.error
            .get_or_insert((decorator.span, "decorators are not supported"));
    }
    fn visit_ts_import_equals_decl(&mut self, import: &TsImportEqualsDecl) {
        self.error.get_or_insert((
            import.span,
            "TypeScript import assignments are not supported",
        ));
    }
    fn visit_ts_export_assignment(&mut self, export: &TsExportAssignment) {
        self.error
            .get_or_insert((export.span, "CommonJS export assignments are not supported"));
    }
    fn visit_import_decl(&mut self, import: &ImportDecl) {
        if import.with.is_some() || import.phase != ImportPhase::Evaluation {
            self.error.get_or_insert((
                import.span,
                "import attributes and phases are not supported",
            ));
        }
    }
    fn visit_named_export(&mut self, export: &NamedExport) {
        if export.with.is_some() {
            self.error
                .get_or_insert((export.span, "export attributes are not supported"));
        }
    }
    fn visit_export_all(&mut self, export: &ExportAll) {
        if export.with.is_some() {
            self.error
                .get_or_insert((export.span, "export attributes are not supported"));
        }
    }
}
