//! Embedding V8: initialize its platform once and create an isolate per run.
//!
//! The host creates and enters a context inside visible handle scopes. Local
//! handles are bound to those scopes; helpers here also define JavaScript
//! properties for integrations without invoking prototype setters.

use std::sync::Once;

static INITIALIZE: Once = Once::new();

pub(crate) fn new_isolate() -> v8::OwnedIsolate {
    INITIALIZE.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });

    v8::Isolate::new(Default::default())
}

pub(crate) fn set_property<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<'s, v8::Object>,
    name: &str,
    value: v8::Local<'s, v8::Value>,
) {
    let key = v8::String::new(scope, name).expect("short property name");
    // Defining own properties avoids script-installed prototype setters.
    assert_eq!(
        object.create_data_property(scope, key.into(), value),
        Some(true)
    );
}
