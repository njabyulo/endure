use runtime::{run_source, Policy, MAX_OUTPUT_BYTES};

#[test]
fn many_small_logs_cannot_exceed_the_combined_output_budget() {
    let error = run_source(&Policy::default(), "logs.js", "try { const chunk='x'.repeat(1023); for(let i=0;i<10000;i++) console.log(chunk); } catch(e) { console.error('caught'); } console.log('continued');").unwrap_err();
    assert_eq!(error.message, "console output limit exceeded");
    assert_eq!(error.output.stdout.len(), MAX_OUTPUT_BYTES);
    assert!(error.output.stderr.is_empty());
    assert!(!error.output.stdout.contains("continued"));
}

#[test]
fn one_huge_value_is_rejected_before_copying_it_to_rust() {
    for value in [
        format!("'x'.repeat({MAX_OUTPUT_BYTES})"),
        format!("'é'.repeat({})", MAX_OUTPUT_BYTES / 2),
    ] {
        let error = run_source(
            &Policy::default(),
            "value.js",
            &format!("console.log('before'); console.log({value});"),
        )
        .unwrap_err();
        assert_eq!(error.message, "console output limit exceeded");
        assert_eq!(error.output.stdout, "before\n");
    }
}

#[test]
fn both_streams_and_arguments_share_one_budget() {
    let source = format!(
        "const chunk='x'.repeat({}); console.log(chunk); console.error(chunk, chunk);",
        MAX_OUTPUT_BYTES / 2
    );
    let error = run_source(&Policy::default(), "streams.js", &source).unwrap_err();
    assert_eq!(error.message, "console output limit exceeded");
    assert_eq!(error.output.stdout.len(), MAX_OUTPUT_BYTES / 2 + 1);
    assert!(error.output.stderr.is_empty());
}

#[test]
fn nested_stringification_logs_are_included_in_the_budget() {
    let source = format!("console.log({{toString(){{console.error('nested'); return 'x'.repeat({MAX_OUTPUT_BYTES});}}}});");
    let error = run_source(&Policy::default(), "nested.js", &source).unwrap_err();
    assert_eq!(error.message, "console output limit exceeded");
    assert_eq!(error.output.stderr, "nested\n");
    assert!(error.output.stdout.is_empty());
}
