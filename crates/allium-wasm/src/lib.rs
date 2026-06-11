use wasm_bindgen::prelude::*;

/// Parse an Allium source string and return the AST as JSON.
#[wasm_bindgen]
pub fn parse(source: &str) -> String {
    let result = allium_parser::parse(source);
    serde_json::to_string(&result)
        .unwrap_or_else(|e| format!(r#"{{"error":"serialisation failed: {e}"}}"#))
}

/// Run semantic analysis over an Allium source string and return diagnostics as JSON.
#[wasm_bindgen]
pub fn analyze(source: &str) -> String {
    let result = allium_parser::parse(source);
    let diagnostics = allium_parser::analyze(&result.module, source);
    serde_json::to_string(&diagnostics)
        .unwrap_or_else(|e| format!(r#"{{"error":"serialisation failed: {e}"}}"#))
}
