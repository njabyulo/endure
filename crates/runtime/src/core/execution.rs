//! Source compilation and execution through V8's Script API.
//!
//! V8 performs parsing, AST construction, bytecode generation, and execution
//! internally. This host supplies source text and a filename, then delegates to
//! V8; it does not implement a separate parser or interpreter.
//! The tests show syntax errors and uncaught exceptions through run_source.

/// Leave JavaScript exceptions pending for the host's surrounding TryCatch.
pub(crate) fn compile_and_run(
    scope: &mut v8::PinScope<'_, '_>,
    filename: &str,
    source: &str,
) -> Result<(), String> {
    let name = v8::String::new(scope, filename)
        .ok_or_else(|| "filename exceeds V8 string limit".to_owned())?;
    let code = v8::String::new(scope, source)
        .ok_or_else(|| "source exceeds V8 string limit".to_owned())?;
    let origin = v8::ScriptOrigin::new(
        scope,
        name.into(),
        0,
        0,
        false,
        -1,
        None,
        false,
        false,
        false,
        None,
    );
    v8::Script::compile(scope, code, Some(&origin))
        .and_then(|script| script.run(scope))
        .map(|_| ())
        .ok_or_else(|| "JavaScript execution failed".to_owned())
}

#[cfg(test)]
mod tests {
    use crate::{run_source, Policy};

    #[test]
    fn syntax_error_reports_filename() {
        let error = run_source(&Policy::default(), "syntax.js", "const = ;").unwrap_err();
        assert_eq!(error.filename, "syntax.js");
        assert!(error.message.contains("SyntaxError"));
    }

    #[test]
    fn uncaught_error_fails() {
        let error = run_source(&Policy::default(), "throw.js", "throw Error('boom');").unwrap_err();
        assert!(error.message.contains("boom"));
    }
}
