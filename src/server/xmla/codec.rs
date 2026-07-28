use axum::http::HeaderMap;

/// LZXPRESS has no built-in output-size limit, so a small compressed payload
/// can otherwise expand into an arbitrarily large allocation. This bounds
/// worst-case decompressed size regardless of how well an input compresses.
const MAX_DECOMPRESSED_BYTES: usize = 256 * 1024 * 1024;

/// Per-request codec derived from the client's negotiation-flags header.
///
/// Flags layout (x-ms-xmlacaps-negotiation-flags: [0],[1],[2],[3],[4]):
///   [0] NEGO  [1] REQ_SX  [2] REQ_XPRESS  [3] RESP_SX  [4] RESP_XPRESS
pub struct XmlaCodec {
    resp_bxml: bool,   // client can receive BXML (RESP_SX)
    resp_xpress: bool, // client can receive LZXPRESS (RESP_XPRESS)
}

impl XmlaCodec {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let flags = headers
            .get("x-ms-xmlacaps-negotiation-flags")
            .and_then(|v| v.to_str().ok())
            .map(parse_flags)
            .unwrap_or([0u8; 5]);

        Self { resp_bxml: flags[3] != 0, resp_xpress: flags[4] != 0 }
    }

    /// Decode a raw HTTP request body into an XML string.
    ///
    /// Handles: plain UTF-8, uncompressed BXML, and LZXPRESS-compressed BXML.
    pub fn decode_request(&self, body: &[u8]) -> String {
        if ms_binxml::is_bxml(body) {
            return match ms_binxml::decode(body) {
                Ok(xml) => xml,
                Err(e) => {
                    tracing::warn!(error = %e, "BXML decode failed, treating as UTF-8");
                    String::from_utf8_lossy(body).into_owned()
                }
            };
        }

        if let Ok(decompressed) = lzxpress::data::decompress(body) {
            if decompressed.len() > MAX_DECOMPRESSED_BYTES {
                tracing::warn!(
                    decompressed_bytes = decompressed.len(),
                    "LZXPRESS output exceeded size cap, rejecting as plain XML"
                );
                return String::from_utf8_lossy(body).into_owned();
            }
            if ms_binxml::is_bxml(&decompressed) {
                return match ms_binxml::decode(&decompressed) {
                    Ok(xml) => xml,
                    Err(e) => {
                        tracing::warn!(error = %e, "BXML decode after decompress failed");
                        String::from_utf8_lossy(body).into_owned()
                    }
                };
            }
        }

        String::from_utf8_lossy(body).into_owned()
    }

    /// Encode an XML string into the response body.
    ///
    /// Returns `(bytes, content-type)`.  Falls back to plain UTF-8 XML if
    /// BXML encoding or LZXPRESS compression fails.
    pub fn encode_response(&self, xml: &str) -> (Vec<u8>, &'static str) {
        if self.resp_bxml {
            match ms_binxml::encode(xml) {
                Ok(bxml) => {
                    if self.resp_xpress {
                        match lzxpress::data::compress(&bxml) {
                            Ok(compressed) => return (compressed, "application/sx+xpress"),
                            Err(e) => tracing::warn!(
                                error = ?e,
                                "LZXPRESS compress failed, sending plain BXML"
                            ),
                        }
                    }
                    return (bxml, "application/sx+xpress");
                }
                Err(e) => tracing::warn!(error = %e, "BXML encode failed, falling back to XML"),
            }
        }
        (xml.as_bytes().to_vec(), "text/xml; charset=utf-8")
    }

    /// The negotiation flags we advertise to clients: we support BXML and
    /// LZXPRESS on both request and response sides.
    pub fn response_flags() -> &'static str {
        "0,1,1,1,1"
    }
}

fn parse_flags(s: &str) -> [u8; 5] {
    let mut out = [0u8; 5];
    for (i, part) in s.split(',').take(5).enumerate() {
        out[i] = part.trim().parse().unwrap_or(0);
    }
    out
}
