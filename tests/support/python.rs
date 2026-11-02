//! Keep direct Python-peer tests below Linux's per-argument size limit.
//! Production streams the helper over stdin; these fixtures reserve stdin for
//! their own request protocol, so compress only the code argument.
pub fn source(source: &str) -> String {
    use base64::Engine;
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(source.as_bytes()).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(encoder.finish().unwrap());
    format!(
        "exec(compile(__import__('zlib').decompress(__import__('base64').b64decode('{encoded}')), '<rode-host>', 'exec'))"
    )
}
