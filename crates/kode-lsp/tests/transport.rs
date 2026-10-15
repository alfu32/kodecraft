use kode_lsp::transport::*;
use serde_json::json;
use tokio::io::AsyncWriteExt;
#[tokio::test]
async fn fragmented_concatenated_frames_use_utf8_bytes_and_accept_case_insensitive_headers() {
    let value = json!({"jsonrpc":"2.0","id":1,"result":"naïve — 😀"});
    let encoded = encode(&value).unwrap();
    let body = serde_json::to_vec(&value).unwrap();
    assert!(encoded.starts_with(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()));
    let (mut writer, reader) = tokio::io::duplex(16);
    let expected = value.clone();
    let task = tokio::spawn(async move {
        for byte in encoded {
            writer.write_all(&[byte]).await.unwrap();
            tokio::task::yield_now().await;
        }
        writer.write_all(format!("content-length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf8\r\nX-Unknown: allowed\r\n\r\n",body.len()).as_bytes()).await.unwrap();
        writer.write_all(&body).await.unwrap();
    });
    let mut reader = FrameReader::new(reader);
    assert_eq!(reader.read().await.unwrap(), Some(expected.clone()));
    assert_eq!(reader.read().await.unwrap(), Some(expected));
    assert!(reader.read().await.unwrap().is_none());
    task.await.unwrap();
}
#[tokio::test]
async fn malformed_oversized_truncated_and_non_json_rpc_input_fails_without_unbounded_header_allocation()
 {
    let mut cases = vec![
        b"Content-Length: 0\r\n\r\n".to_vec(),
        b"Content-Length: -1\r\n\r\n".to_vec(),
        b"Content-Length: 1\r\nContent-Length: 1\r\n\r\n0".to_vec(),
        b"Unknown: field\r\n\r\n".to_vec(),
        b"Content-Length: 10\n\n".to_vec(),
        b"Content-Length: 10\r\n\r\nshort".to_vec(),
        b"Content-Length: 2\r\n\r\n{}".to_vec(),
        b"Content-Length: 2\r\nContent-Type: application/json; charset=utf16\r\n\r\n{}".to_vec(),
        b"Content-Length: 2\r\n\r\n\xff\xff".to_vec(),
    ];
    cases.push(format!("Content-Length: {}\r\n\r\n", MAX_FRAME_BYTES + 1).into_bytes());
    cases.push(vec![b'x'; MAX_HEADER_BYTES + 1]);
    for (index, bytes) in cases.iter().enumerate() {
        assert!(
            FrameReader::new(bytes.as_slice()).read().await.is_err(),
            "case {index}"
        );
    }
    assert!(FrameReader::new(&b""[..]).read().await.unwrap().is_none());
    assert!(encode(&json!([1, 2])).is_err());
}
