use flutz_formats::builtin_registry;
use serde_json::json;

fn main() {
    let registry = builtin_registry();
    for descriptor in registry.descriptors() {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "format": descriptor.id,
                "friendly_name": descriptor.friendly_name,
                "extensions": descriptor.extensions,
                "wrapped_extensions": descriptor.wrapped_extensions,
                "backend": descriptor.backend.as_str(),
                "content_kind": descriptor.content_kind.as_str(),
                "mastering": descriptor.mastering.as_str(),
                "supports_metadata": descriptor.supports_metadata,
                "supports_looping": descriptor.supports_looping,
                "status": "ok",
            }))
            .expect("registry record serializes")
        );
    }
    println!(
        "{}",
        serde_json::to_string(&json!({
            "scenario": "format-registry",
            "format_count": registry.descriptors().len(),
            "status": "ok",
        }))
        .expect("summary record serializes")
    );
}
