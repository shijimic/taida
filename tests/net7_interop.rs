/// NET7-7e: HTTP/3 interop integration test.
/// Uses quinn (QUIC transport) for local H3 server/client roundtrip.
/// Pure Rust — no external tools required, CI-compatible.
///
/// This test validates:
/// 1. QUIC connection (TLS 1.3 + ALPN h3 negotiation)
/// 2. Bidirectional stream I/O
/// 3. H3-style frame encoding/decoding over QUIC streams

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};

/// Initialize the default crypto provider. Must be called before any crypto config.
fn init_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

// ── Test certificate helpers ──

fn make_test_cert() -> (Vec<u8>, Vec<u8>) {
    let key_pair = KeyPair::generate().expect("generate key pair");
    let mut params = CertificateParams::new(vec!["localhost".to_string()]).expect("cert params");
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, "taida-test");

    let cert = params.self_signed(&key_pair).expect("self sign");
    (cert.der().to_vec(), key_pair.serialize_der())
}

fn make_server_crypto(cert_der: &[u8], key_der: &[u8]) -> Arc<QuicServerConfig> {
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(key_der.to_vec());
    let cert = rustls::pki_types::CertificateDer::from(cert_der.to_vec());

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key.into())
        .expect("single cert");
    server_crypto.alpn_protocols = vec![b"h3".to_vec()];

    Arc::new(QuicServerConfig::try_from(server_crypto).expect("quic server config"))
}

fn build_server_config(cert_der: &[u8], key_der: &[u8]) -> quinn::ServerConfig {
    let crypto = make_server_crypto(cert_der, key_der);
    let mut server_config = quinn::ServerConfig::with_crypto(crypto);

    let transport = Arc::new({
        let mut config = quinn::TransportConfig::default();
        config
            .max_concurrent_bidi_streams(1u32.into())
            .max_concurrent_uni_streams(1u32.into());
        config
    });
    server_config.transport_config(transport);
    server_config
}

/// SkipServerVerification: accepts any server certificate (for testing only).
#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

fn build_client_config() -> quinn::ClientConfig {
    let mut crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"h3".to_vec()];

    let quic_config = QuicClientConfig::try_from(crypto).expect("quic client config");
    quinn::ClientConfig::new(Arc::new(quic_config))
}

fn build_client_config_with_cert(cert_der: &[u8]) -> quinn::ClientConfig {
    let mut root_store = rustls::RootCertStore::empty();
    root_store
        .add(rustls::pki_types::CertificateDer::from(cert_der.to_vec()))
        .expect("add cert");

    let mut crypto = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"h3".to_vec()];

    let quic_config = QuicClientConfig::try_from(crypto).expect("quic client config");
    quinn::ClientConfig::new(Arc::new(quic_config))
}

// ── H3-style frame encoding (simplified) ──

fn encode_varint(value: u64) -> Vec<u8> {
    if value < 0x40 {
        vec![value as u8]
    } else if value < 0x4000 {
        vec![0x40 | (value >> 8) as u8, value as u8]
    } else if value < 0x4000_0000 {
        vec![
            0x80 | (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ]
    } else {
        vec![
            0xC0 | (value >> 56) as u8,
            (value >> 48) as u8,
            (value >> 40) as u8,
            (value >> 32) as u8,
            (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ]
    }
}

fn h3_settings_frame() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(encode_varint(0x01));
    body.extend(encode_varint(0x00));
    body.extend(encode_varint(0x07));
    body.extend(encode_varint(0x00));
    body.extend(encode_varint(0x06));
    body.extend(encode_varint(65536));

    let mut frame = encode_varint(0x04);
    frame.extend(encode_varint(body.len() as u64));
    frame.extend(body);
    frame
}

fn h3_headers_frame(block: &[u8]) -> Vec<u8> {
    let mut frame = encode_varint(0x01);
    frame.extend(encode_varint(block.len() as u64));
    frame.extend_from_slice(block);
    frame
}

fn h3_data_frame(body: &[u8]) -> Vec<u8> {
    let mut frame = encode_varint(0x00);
    frame.extend(encode_varint(body.len() as u64));
    frame.extend_from_slice(body);
    frame
}

fn h3_request_headers() -> Vec<u8> {
    vec![
        0x00, 7, b':', b'm', b'e', b't', b'h', b'o', b'd',
        3, b'G', b'E', b'T',
        0x00, 5, b':', b'p', b'a', b't', b'h',
        5, b'/', b't', b'e', b's', b't',
        0x00, 7, b':', b's', b'c', b'h', b'e', b'm', b'e',
        5, b'h', b't', b't', b'p', b's',
        0x00, 11, b':', b'a', b'u', b't', b'h', b'o', b'r', b'i', b't', b'y',
        9, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't',
    ]
}

// ── Integration Tests ──

/// NET7-7e: QUIC transport connection + H3-style frame roundtrip.
#[tokio::test]
async fn test_h3_interop_quic_connect() {
    init_crypto();

    let (cert_der, key_der) = make_test_cert();

    let server_endpoint = quinn::Endpoint::server(
        build_server_config(&cert_der, &key_der),
        SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
    )
    .expect("bind server");

    let server_addr = server_endpoint.local_addr().expect("local addr");
    let server_cert_der = Arc::new(cert_der.clone());

    // Server: accept and respond
    let server_handle = tokio::spawn(async move {
        let conn = server_endpoint
            .accept()
            .await
            .expect("accept connection")
            .await
            .expect("establish");

        let (mut send, mut recv) = conn
            .accept_bi()
            .await
            .expect("accept bi stream");

        // Read request
        let mut buf = [0u8; 4096];
        let n = recv.read(&mut buf).await.expect("read").expect("data");

        // Verify request contains valid H3 frames
        let mut pos = 0;
        while pos < n {
            let (frame_type, consumed) = decode_varint(&buf[pos..]).expect("frame type");
            pos += consumed;
            let (frame_len, consumed) = decode_varint(&buf[pos..]).expect("frame length");
            pos += consumed;
            if pos + frame_len as usize > n {
                break;
            }
            pos += frame_len as usize;
            assert!(
                matches!(frame_type, 0x00 | 0x01 | 0x03 | 0x04),
                "Unexpected frame type: 0x{:x}",
                frame_type
            );
        }

        // Respond: HEADERS (status 200) + DATA
        let response_headers = vec![0xF9]; // :status=200 (static table index 25)
        let headers_frame = h3_headers_frame(&response_headers);
        let data_frame = h3_data_frame(b"Hello from H3 server");

        send.write_all(&headers_frame).await.expect("write headers");
        send.write_all(&data_frame).await.expect("write data");
        send.finish().expect("finish");

        // Give the client time to read our data before closing the connection
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        conn.close(0u32.into(), b"done");
    });

    // Client: connect and send request
    let mut client_endpoint = quinn::Endpoint::client(SocketAddr::new(
        std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        0,
    ))
    .expect("bind client");

    client_endpoint.set_default_client_config(build_client_config_with_cert(&server_cert_der));

    let conn = client_endpoint
        .connect(server_addr, "localhost")
        .expect("connect")
        .await
        .expect("establish");

    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .expect("open bi stream");

    // Send SETTINGS + request
    send.write_all(&h3_settings_frame())
        .await
        .expect("write settings");
    send.write_all(&h3_headers_frame(&h3_request_headers()))
        .await
        .expect("write request");
    send.finish().expect("finish");

    // Read response: read_to_end waits for send.finish() from server
    let mut result = Vec::new();
    let mut buf = [0u8; 4096];
    while let Some(n) = recv.read(&mut buf).await.expect("read") {
        result.extend_from_slice(&buf[..n]);
    }
    let response = &result[..];

    // Parse response: HEADERS + DATA
    let mut pos = 0;
    let (headers_type, consumed) = decode_varint(&response[pos..]).expect("response frame type");
    assert_eq!(headers_type, 0x01, "First frame should be HEADERS");
    pos += consumed;
    let (headers_len, consumed) =
        decode_varint(&response[pos..]).expect("response frame length");
    pos += consumed + headers_len as usize;

    let (data_type, consumed) =
        decode_varint(&response[pos..]).expect("response data type");
    assert_eq!(data_type, 0x00, "Second frame should be DATA");
    pos += consumed;
    let (data_len, _) = decode_varint(&response[pos..]).expect("response data length");
    pos += consumed;

    let body = &response[pos..pos + data_len as usize];
    assert_eq!(body, b"Hello from H3 server", "response body mismatch");

    conn.close(0u32.into(), b"test complete");
    server_handle.await.expect("server task");
}

fn decode_varint(buf: &[u8]) -> Option<(u64, usize)> {
    if buf.is_empty() {
        return None;
    }
    let len = match buf[0] >> 6 {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        _ => unreachable!(),
    };
    if buf.len() < len {
        return None;
    }
    let mask: u64 = match len {
        1 => 0x3F,
        2 => 0x3FFF,
        4 => 0x3FFF_FFFF,
        8 => 0x3FFF_FFFF_FFFF_FFFF,
        _ => unreachable!(),
    };
    let mut value = (buf[0] as u64) & mask;
    for i in 1..len {
        value = (value << 8) | buf[i] as u64;
    }
    Some((value, len))
}

/// Verify H3 frame encoding/decoding.
#[test]
fn test_h3_frame_roundtrip() {
    let settings = h3_settings_frame();
    let (frame_type, _) = decode_varint(&settings).expect("settings type");
    assert_eq!(frame_type, 0x04);

    let headers = h3_headers_frame(&[0xFF, 0x00]);
    let (frame_type, _) = decode_varint(&headers).expect("headers type");
    assert_eq!(frame_type, 0x01);

    let data = h3_data_frame(b"hello");
    let (frame_type, _) = decode_varint(&data).expect("data type");
    assert_eq!(frame_type, 0x00);
}

/// Verify quinn endpoint creation works.
#[tokio::test]
async fn test_quinn_endpoint() {
    init_crypto();
    let config = build_client_config();
    let mut endpoint = quinn::Endpoint::client(SocketAddr::new(
        std::net::IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        0,
    ))
    .expect("bind");
    endpoint.set_default_client_config(config);
    assert!(endpoint.local_addr().is_ok());
}

/// Verify self-signed cert generation.
#[test]
fn test_cert_generation() {
    let (cert, key) = make_test_cert();
    assert!(!cert.is_empty());
    assert!(!key.is_empty());
    assert_eq!(cert[0], 0x30); // ASN.1 SEQUENCE
}
