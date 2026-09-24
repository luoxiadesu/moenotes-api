fn main() {
    println!(
        "{}",
        serde_json::to_string_pretty(&moenotes_server::openapi_document(
            moenotes_server::projection::ResponseMode::Public
        ))
        .unwrap()
    );
}
