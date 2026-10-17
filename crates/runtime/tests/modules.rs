mod support;

use runtime::{run_module, ModuleGraph, Policy};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use support::Witness;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ondure-modules-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, source: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
    fn load(&self) -> ModuleGraph {
        ModuleGraph::load(&self.0.join("main.ts")).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn typescript_imports_javascript_and_reexports_with_top_level_await() {
    let project = Project::new();
    project.write("main.ts", "import greet, {answer} from './barrel.ts'; const result: string = await new Promise(resolve => setTimeout(() => resolve(greet(answer)), 1)); console.log(result);");
    project.write(
        "barrel.ts",
        "export {default} from './greet.js'; export * from './answer.ts';",
    );
    project.write("greet.js", "export default value => `answer ${value}`;");
    project.write("answer.ts", "interface Shape {value: number}; const data: Shape = {value: 42}; export const answer: number = data.value;");
    assert_eq!(
        run_module(&Policy::default(), &project.load(), None)
            .unwrap()
            .stdout,
        "answer 42\n"
    );
}

#[test]
fn shared_dependencies_execute_once_and_cycles_link() {
    let project = Project::new();
    project.write("main.ts", "import {left} from './left.ts'; import {right} from './right.ts'; console.log(left(), right());");
    project.write("left.ts", "import './shared.js'; import {right} from './right.ts'; export function left(): string { return 'L' + right(); }");
    project.write("right.ts", "import './shared.js'; import {left} from './left.ts'; export function right(): string { return 'R'; } export const reference = () => left;");
    project.write("shared.js", "console.log('shared');");
    assert_eq!(
        run_module(&Policy::default(), &project.load(), None)
            .unwrap()
            .stdout,
        "shared\nLR R\n"
    );
}

#[test]
fn type_only_imports_are_erased_without_loading_files() {
    let project = Project::new();
    project.write("main.ts", "import type {Missing} from './missing.ts'; import {type Other} from './other.ts'; const value: Missing = 42; console.log(value);");
    // Verbatim TS semantics retain this import's module side effects even
    // though its inline type binding is erased.
    project.write(
        "other.ts",
        "console.log('side effect'); export type Other = string;",
    );
    assert_eq!(
        run_module(&Policy::default(), &project.load(), None)
            .unwrap()
            .stdout,
        "side effect\n42\n"
    );
}

#[test]
fn snapshot_and_hash_are_stable_and_changes_fail_verification() {
    let project = Project::new();
    let copy = Project::new();
    for target in [&project, &copy] {
        target.write(
            "main.ts",
            "import {value} from './dependency.ts'; console.log(value);",
        );
        target.write("dependency.ts", "export const value: number = 1;");
    }
    let graph = project.load();
    let original = graph.code_hash().to_owned();
    assert_eq!(original.len(), 64);
    assert_eq!(original, project.load().code_hash());
    assert_eq!(original, copy.load().code_hash());
    project.write("dependency.ts", "export const value: number = 2;");
    assert_eq!(
        run_module(&Policy::default(), &graph, Some(&original))
            .unwrap()
            .stdout,
        "1\n"
    );
    let changed = project.load();
    assert_ne!(original, changed.code_hash());
    let error = run_module(&Policy::default(), &changed, Some(&original)).unwrap_err();
    assert!(error.message.contains(&original));
    assert!(error.message.contains(changed.code_hash()));
    assert!(error.output.stdout.is_empty());
    let dependency_hash = changed.code_hash().to_owned();
    project.write(
        "main.ts",
        "import {value} from './dependency.ts'; console.log('changed', value);",
    );
    assert_ne!(dependency_hash, project.load().code_hash());
    // Deletion after preparation cannot cause a disk read during execution.
    fs::remove_dir_all(&project.0).unwrap();
    assert_eq!(
        run_module(&Policy::default(), &graph, None).unwrap().stdout,
        "1\n"
    );
}

#[test]
fn hash_mismatch_prevents_network_and_timers() {
    let project = Project::new();
    let witness = Witness::new(200, r#"{"hit":1}"#);
    project.write(
        "main.ts",
        &format!(
            "console.log('wrong'); setTimeout(() => console.log('timer')); await fetch('{}');",
            witness.url()
        ),
    );
    let graph = project.load();
    let error = run_module(&Policy { net: true }, &graph, Some("previous-hash")).unwrap_err();
    assert!(error.message.contains("code hash mismatch"));
    assert!(error.output.stdout.is_empty());
    assert_eq!(witness.finish(), 0);
}

#[test]
fn unsupported_imports_and_syntax_fail_preparation() {
    for (source, expected) in [
        ("import './missing.ts';", "cannot resolve"),
        ("import './dependency';", "explicit"),
        ("import 'package';", "unsupported import"),
        (
            "import 'https://example.com/code.js';",
            "unsupported import",
        ),
        ("import '/absolute.js';", "unsupported import"),
        ("import './data.json';", "explicit"),
        ("import('./dependency.ts');", "dynamic imports"),
        (
            "import thing = require('./dependency.ts');",
            "import assignments",
        ),
        ("export = 1;", "export assignments"),
        ("const view = <div/>;", "SyntaxError"),
        ("@example class Thing {}", "SyntaxError"),
        (
            "import './dependency.ts' with {type: 'json'};",
            "attributes",
        ),
    ] {
        let project = Project::new();
        project.write(
            "main.ts",
            &format!("console.log('must not execute'); {source}"),
        );
        let error = ModuleGraph::load(&project.0.join("main.ts")).unwrap_err();
        assert!(error.message.contains(expected), "{source}: {error}");
    }
}

#[test]
fn paths_and_symlinks_cannot_escape_the_entry_directory() {
    let project = Project::new();
    project.write("outside.ts", "console.log('wrong');");
    project.write("src/main.ts", "import '../outside.ts';");
    let error = ModuleGraph::load(&project.0.join("src/main.ts")).unwrap_err();
    assert!(error.message.contains("escapes entry directory"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(project.0.join("outside.ts"), project.0.join("src/link.ts"))
            .unwrap();
        project.write("src/main.ts", "import './link.ts';");
        let error = ModuleGraph::load(&project.0.join("src/main.ts")).unwrap_err();
        assert!(error.message.contains("escapes entry directory"));
    }
}

#[test]
fn dependencies_can_use_parent_paths_within_root_and_aliases_share_identity() {
    let project = Project::new();
    project.write("main.ts", "import './sub/child.ts'; import './shared.ts';");
    project.write("sub/child.ts", "import '../shared.ts';");
    project.write("shared.ts", "console.log('once');");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(project.0.join("shared.ts"), project.0.join("alias.ts"))
            .unwrap();
        project.write(
            "main.ts",
            "import './sub/child.ts'; import './shared.ts'; import './alias.ts';",
        );
    }
    assert_eq!(
        run_module(&Policy::default(), &project.load(), None)
            .unwrap()
            .stdout,
        "once\n"
    );
}

#[test]
fn preparation_errors_identify_dependency_and_location() {
    let project = Project::new();
    project.write("main.ts", "import './broken.ts';");
    project.write("broken.ts", "\nconst = ;");
    let error = ModuleGraph::load(&project.0.join("main.ts")).unwrap_err();
    assert_eq!(error.filename, "broken.ts");
    assert!(error.message.contains("SyntaxError"));
    assert!(error.message.contains("(2:"));
}

#[test]
fn module_errors_retain_output_and_drain_submitted_work() {
    let project = Project::new();
    for source in [
        "console.log('starting'); setTimeout(() => console.error('done')); throw Error('boom');",
        "console.log('starting'); await new Promise(resolve => setTimeout(resolve)); setTimeout(() => console.error('done')); throw Error('boom');",
    ] {
        project.write("main.ts", source);
        let error = run_module(&Policy::default(), &project.load(), None).unwrap_err();
        assert_eq!(error.message, "Error: boom");
        assert_eq!(error.output.stdout, "starting\n");
        assert_eq!(error.output.stderr, "done\n");
    }
}

#[test]
fn pending_evaluation_at_idle_is_an_error() {
    let project = Project::new();
    project.write(
        "main.ts",
        "console.log('starting'); await new Promise(() => {});",
    );
    let error = run_module(&Policy::default(), &project.load(), None).unwrap_err();
    assert!(error.message.contains("module evaluation is pending"));
    assert_eq!(error.output.stdout, "starting\n");
}

#[test]
fn missing_exports_fail_linking_before_side_effects() {
    let project = Project::new();
    project.write(
        "main.ts",
        "import {missing} from './dep.js'; console.log(missing);",
    );
    project.write("dep.js", "console.log('wrong'); export const value = 1;");
    let error = run_module(&Policy::default(), &project.load(), None).unwrap_err();
    assert!(error.message.contains("missing"));
    assert!(error.output.stdout.is_empty());
}

#[test]
fn top_level_await_fetch_obeys_policy_and_works_from_tokio() {
    let project = Project::new();
    let witness = Witness::new(200, r#"{"hit":1}"#);
    project.write(
        "main.ts",
        &format!(
            "const response = await fetch('{}'); console.log((await response.json()).hit);",
            witness.url()
        ),
    );
    let graph = project.load();
    let error = run_module(&Policy::default(), &graph, None).unwrap_err();
    assert!(error.message.contains("permission denied: net"));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let output = runtime
        .block_on(async { run_module(&Policy { net: true }, &graph, None) })
        .unwrap();
    assert_eq!(output.stdout, "1\n");
    assert_eq!(witness.finish(), 1);
}

#[test]
fn eval_cannot_bypass_dynamic_import_restriction() {
    let project = Project::new();
    project.write(
        "main.ts",
        "try { await eval(\"import('./unprepared.js')\"); } catch(e) { console.log(e.message); }",
    );
    assert_eq!(
        run_module(&Policy::default(), &project.load(), None)
            .unwrap()
            .stdout,
        "dynamic imports are not supported\n"
    );
}

#[test]
fn rendering_module_errors_cannot_hide_new_microtasks() {
    let project = Project::new();
    project.write("main.ts", "throw {toString() { Promise.resolve().then(() => setTimeout(() => console.log('done'))); return 'reason'; }};");
    let error = run_module(&Policy::default(), &project.load(), None).unwrap_err();
    assert_eq!(error.message, "reason");
    assert_eq!(error.output.stdout, "done\n");
}

#[test]
fn typescript_runtime_constructs_are_transformed() {
    let project = Project::new();
    project.write("main.ts", "enum State { Ready = 7 }; class Job { constructor(public state: State) {} }; console.log(new Job(State.Ready).state);");
    assert_eq!(
        run_module(&Policy::default(), &project.load(), None)
            .unwrap()
            .stdout,
        "7\n"
    );
}

#[test]
fn dependency_top_level_await_finishes_before_importer_runs() {
    let project = Project::new();
    project.write(
        "main.ts",
        "import {value} from './dep.ts'; console.log('entry', value);",
    );
    project.write("dep.ts", "export const value: number = await new Promise(resolve => setTimeout(resolve, 1, 42)); console.log('dependency');");
    assert_eq!(
        run_module(&Policy::default(), &project.load(), None)
            .unwrap()
            .stdout,
        "dependency\nentry 42\n"
    );
}

#[test]
fn non_unicode_specifiers_and_declaration_entries_are_rejected() {
    let project = Project::new();
    project.write("main.ts", r"import './\ud800.ts';");
    let error = ModuleGraph::load(&project.0.join("main.ts")).unwrap_err();
    assert!(error.message.contains("valid Unicode"));
    project.write("types.d.ts", "declare const value: number;");
    let error = ModuleGraph::load(&project.0.join("types.d.ts")).unwrap_err();
    assert!(error.message.contains("only executable .js and .ts"));
}
