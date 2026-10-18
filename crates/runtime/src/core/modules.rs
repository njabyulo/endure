//! Capture a local module graph before execution and link V8 from that snapshot.
//! Paths identify code inside the entry directory; no module is read at runtime.

use super::transpilation::transpile;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, fs, path::Path};

#[derive(Debug)]
pub struct ModuleError {
    pub filename: String,
    pub message: String,
}
impl ModuleError {
    pub(crate) fn new(filename: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            filename: filename.into(),
            message: message.into(),
        }
    }
}
impl fmt::Display for ModuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.filename, self.message)
    }
}
impl std::error::Error for ModuleError {}

/// An immutable snapshot of the reachable execution graph, including emitted JS.
/// Explicit `import type` declarations are erased before graph traversal.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ModuleGraph {
    pub(crate) filename: String,
    entry: String,
    modules: BTreeMap<String, PreparedModule>,
    hash: String,
}
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PreparedModule {
    source: String,
    javascript: String,
    imports: BTreeMap<String, String>,
}

impl ModuleGraph {
    /// Read and prepare local .js/.ts modules under the entry's canonical parent.
    /// Preparation is not an atomic snapshot of concurrent filesystem changes.
    pub fn load(entry: &Path) -> Result<Self, ModuleError> {
        let filename = entry.to_string_lossy().into_owned();
        let path =
            fs::canonicalize(entry).map_err(|e| ModuleError::new(&filename, e.to_string()))?;
        let root = path
            .parent()
            .ok_or_else(|| ModuleError::new(&filename, "entry has no parent directory"))?;
        let mut graph = Self {
            filename,
            entry: String::new(),
            modules: BTreeMap::new(),
            hash: String::new(),
        };
        graph.entry = graph.load_module(root, &path)?;
        graph.hash = graph.compute_hash();
        Ok(graph)
    }

    /// SHA-256 of the versioned graph, independent of its absolute checkout path.
    pub fn code_hash(&self) -> &str {
        &self.hash
    }

    fn load_module(&mut self, root: &Path, path: &Path) -> Result<String, ModuleError> {
        let relative = path.strip_prefix(root).map_err(|_| {
            ModuleError::new(path.display().to_string(), "module escapes entry directory")
        })?;
        let id = relative
            .to_str()
            .ok_or_else(|| {
                ModuleError::new(path.display().to_string(), "module path must be UTF-8")
            })?
            .replace(std::path::MAIN_SEPARATOR, "/");
        if self.modules.contains_key(&id) {
            return Ok(id);
        }
        let extension = path.extension().and_then(|s| s.to_str());
        if !matches!(extension, Some("js" | "ts")) || id.ends_with(".d.ts") {
            return Err(ModuleError::new(
                &id,
                "only executable .js and .ts modules are supported",
            ));
        }
        let source = fs::read_to_string(path).map_err(|e| ModuleError::new(&id, e.to_string()))?;
        let prepared = transpile(&id, &source, extension == Some("ts"))?;
        // Insert before following edges so cycles refer to an existing module.
        self.modules.insert(
            id.clone(),
            PreparedModule {
                source,
                javascript: prepared.javascript,
                imports: BTreeMap::new(),
            },
        );
        for specifier in prepared.imports {
            if specifier == "ondure" {
                self.modules
                    .get_mut(&id)
                    .expect("prepared module")
                    .imports
                    .insert(specifier, "ondure".into());
                continue;
            }
            if !(specifier.starts_with("./") || specifier.starts_with("../"))
                || specifier.contains(['\\', '?', '#'])
            {
                return Err(ModuleError::new(
                    &id,
                    format!("unsupported import {specifier:?}; use a relative .js or .ts path"),
                ));
            }
            let requested = path.parent().expect("module parent").join(&specifier);
            if !matches!(
                requested.extension().and_then(|s| s.to_str()),
                Some("js" | "ts")
            ) {
                return Err(ModuleError::new(
                    &id,
                    format!("import {specifier:?} requires an explicit .js or .ts extension"),
                ));
            }
            let target = fs::canonicalize(&requested)
                .map_err(|e| ModuleError::new(&id, format!("cannot resolve {specifier:?}: {e}")))?;
            let target_id = self.load_module(root, &target)?;
            self.modules
                .get_mut(&id)
                .expect("prepared module")
                .imports
                .insert(specifier, target_id);
        }
        Ok(id)
    }

    pub(crate) fn verify_integrity(&self) -> Result<(), ModuleError> {
        if !self.modules.contains_key(&self.entry)
            || self.modules.values().any(|module| {
                module
                    .imports
                    .values()
                    .any(|id| id != "ondure" && !self.modules.contains_key(id))
            })
            || self.compute_hash() != self.hash
        {
            return Err(ModuleError::new(
                &self.filename,
                "module snapshot integrity check failed",
            ));
        }
        Ok(())
    }

    pub(crate) fn uses_agent_module(&self) -> bool {
        self.modules
            .values()
            .any(|m| m.imports.values().any(|id| id == "ondure"))
    }

    fn compute_hash(&self) -> String {
        fn field(hash: &mut Sha256, bytes: &[u8]) {
            hash.update((bytes.len() as u64).to_be_bytes());
            hash.update(bytes);
        }
        let mut hash = Sha256::new();
        field(&mut hash, b"ondure-module-graph-v1");
        field(&mut hash, self.entry.as_bytes());
        hash.update((self.modules.len() as u64).to_be_bytes());
        for (id, module) in &self.modules {
            field(&mut hash, id.as_bytes());
            field(&mut hash, module.source.as_bytes());
            field(&mut hash, module.javascript.as_bytes());
            hash.update((module.imports.len() as u64).to_be_bytes());
            for (specifier, target) in &module.imports {
                field(&mut hash, specifier.as_bytes());
                field(&mut hash, target.as_bytes());
            }
        }
        format!("{:x}", hash.finalize())
    }
}

// Each isolate owns its compiled modules. Resolver callbacks only consult this
// registry and never perform I/O or borrow host state while executing JS.
#[derive(Default)]
pub(crate) struct ModuleStore {
    compiled: BTreeMap<String, v8::Global<v8::Module>>,
    edges: BTreeMap<i32, BTreeMap<String, String>>,
}

pub(crate) fn evaluate(
    scope: &mut v8::PinScope<'_, '_>,
    graph: &ModuleGraph,
) -> Result<v8::Global<v8::Promise>, String> {
    let mut store = ModuleStore::default();
    for (id, module) in &graph.modules {
        let name = v8::String::new(scope, id).ok_or("module filename exceeds V8 string limit")?;
        let code = v8::String::new(scope, &module.javascript)
            .ok_or("module source exceeds V8 string limit")?;
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
            true,
            None,
        );
        let mut source = v8::script_compiler::Source::new(code, Some(&origin));
        let compiled = v8::script_compiler::compile_module(scope, &mut source)
            .ok_or_else(|| format!("could not compile {id}"))?;
        store
            .compiled
            .insert(id.clone(), v8::Global::new(scope, compiled));
        let script_id = compiled
            .script_id()
            .ok_or("compiled module has no script ID")?;
        store.edges.insert(script_id, module.imports.clone());
    }
    if graph.uses_agent_module() {
        let name = v8::String::new(scope, "ondure").ok_or("builtin module name")?;
        let export = v8::String::new(scope, "agent").ok_or("builtin export")?;
        let builtin = v8::Module::create_synthetic_module(
            scope,
            name,
            &[export],
            crate::capabilities::agent::evaluate_builtin,
        );
        store
            .compiled
            .insert("ondure".into(), v8::Global::new(scope, builtin));
    }
    let entry = v8::Local::new(scope, &store.compiled[&graph.entry]);
    scope.set_slot(store);
    if entry.instantiate_module(scope, resolve) != Some(true) {
        return Err("module linking failed".into());
    }
    let value = entry.evaluate(scope).ok_or("module evaluation failed")?;
    let promise = v8::Local::<v8::Promise>::try_from(value)
        .map_err(|_| "module evaluation did not return a promise")?;
    let promise = if graph.uses_agent_module() {
        let namespace = entry.get_module_namespace();
        let invoke = v8::Function::builder(crate::capabilities::agent::invoke_default)
            .data(namespace)
            .build(scope)
            .ok_or("agent invocation callback")?;
        promise
            .then(scope, invoke)
            .ok_or("agent invocation continuation")?
    } else {
        promise
    };
    // A real rejection handler clears an already-reported rejection and marks
    // future rejection handled. The event loop observes the original promise between completions.
    let handler = v8::Function::new(
        scope,
        |_scope: &mut v8::PinScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue| {},
    )
    .expect("evaluation rejection handler");
    promise
        .catch(scope, handler)
        .ok_or("could not observe module evaluation")?;
    Ok(v8::Global::new(scope, promise))
}

fn resolve<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _attributes: v8::Local<'s, v8::FixedArray>,
    referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    // SAFETY: V8 enters this callback with its context and isolate active.
    v8::callback_scope!(unsafe scope, context);
    let specifier = specifier.to_rust_string_lossy(scope);
    let retained = scope.get_slot::<ModuleStore>().and_then(|store| {
        let target = store.edges.get(&referrer.script_id()?)?.get(&specifier)?;
        store.compiled.get(target).cloned()
    });
    let Some(retained) = retained else {
        let message = v8::String::new(scope, &format!("prepared import not found: {specifier}"))?;
        let error = v8::Exception::error(scope, message);
        scope.throw_exception(error);
        return None;
    };
    Some(v8::Local::new(scope, &retained))
}

/// Even import expressions constructed through eval cannot read outside the
/// prepared graph. This callback only returns a rejected promise; it does no I/O.
pub(crate) fn reject_dynamic_import<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _options: v8::Local<'s, v8::Data>,
    _resource: v8::Local<'s, v8::Value>,
    _specifier: v8::Local<'s, v8::String>,
    _attributes: v8::Local<'s, v8::FixedArray>,
) -> Option<v8::Local<'s, v8::Promise>> {
    let resolver = v8::PromiseResolver::new(scope)?;
    let message = v8::String::new(scope, "dynamic imports are not supported")?;
    let error = v8::Exception::error(scope, message);
    resolver.reject(scope, error)?;
    Some(resolver.get_promise(scope))
}
