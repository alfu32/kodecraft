use kode_database::bridge::protocol::*;
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, duplex};
#[test]
fn every_documented_method_has_a_shared_wire_fixture_and_schema_definition() {
    let schema: Value = serde_json::from_slice(include_bytes!(
        "../../../protocol/jdbc-v1/operations.schema.json"
    ))
    .unwrap();
    let fixtures: Value = serde_json::from_slice(include_bytes!(
        "../../../protocol/jdbc-v1/operation-fixtures.json"
    ))
    .unwrap();
    let mut covered = std::collections::BTreeSet::new();
    for value in fixtures["valid"].as_array().unwrap() {
        let frame = encode(value).unwrap();
        assert_eq!(decode(&frame[4..]).unwrap(), *value);
        let method = value["method"].as_str().unwrap();
        let definition = schema["x-methods"][method].as_str().unwrap();
        assert!(schema["$defs"].get(definition).is_some());
        covered.insert(method);
    }
    assert_eq!(
        covered,
        schema["x-methods"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    );
}
#[test]
fn shared_query_fixtures_keep_null_empty_binary_unicode_and_decimal_strings_distinct() {
    use kode_database::bridge::wire::{Cell, QueryEvent, QueryFinal, QueryNotification};
    let fixture: Value = serde_json::from_slice(include_bytes!(
        "../../../protocol/jdbc-v1/query-fixtures.json"
    ))
    .unwrap();
    for (sequence, value) in fixture["events"].as_array().unwrap().iter().enumerate() {
        let event: QueryNotification = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(event.sequence, sequence as u64);
        let envelope = json!({"jsonrpc":"2.0","method":"query.event","params":serde_json::to_value(&event).unwrap()});
        assert_eq!(decode(&encode(&envelope).unwrap()[4..]).unwrap(), envelope);
        if let QueryEvent::Rows { rows, .. } = event.event {
            assert_eq!(rows[0][0], Cell::Null);
            assert!(matches!(&rows[1][0],Cell::Text {text,..} if text.is_empty()));
            assert!(matches!(&rows[0][1],Cell::Binary {base64,..} if base64=="AP8="));
            assert!(
                matches!(&rows[2][0],Cell::Text {text,..} if text=="12345678901234567890.000000000000000001")
            );
        }
    }
    let final_result: QueryFinal = serde_json::from_value(fixture["final"].clone()).unwrap();
    assert!(final_result.transaction_usable);
    assert_eq!(
        serde_json::to_value(final_result).unwrap(),
        fixture["final"]
    );
}
#[tokio::test]
async fn shared_fixtures_fragmented_adjacent_frames_and_utf8_byte_lengths_roundtrip() {
    let fixture: Value =
        serde_json::from_slice(include_bytes!("../../../protocol/jdbc-v1/fixtures.json")).unwrap();
    let valid = fixture["valid"].as_array().unwrap();
    let mut all = Vec::new();
    for value in valid {
        let frame = encode(value).unwrap();
        assert_eq!(
            u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize,
            frame.len() - 4
        );
        assert_eq!(decode(&frame[4..]).unwrap(), *value);
        all.extend(frame);
    }
    for value in fixture["invalid"].as_array().unwrap() {
        assert!(validate(value).is_err());
    }
    let (mut send, receive) = duplex(23);
    let task = tokio::spawn(async move {
        for byte in all {
            send.write_all(&[byte]).await.unwrap();
        }
    });
    let mut reader = FrameReader::new(receive);
    for value in valid {
        assert_eq!(reader.read().await.unwrap().as_ref(), Some(value));
    }
    assert!(reader.read().await.unwrap().is_none());
    task.await.unwrap();
}
#[tokio::test]
async fn malformed_lengths_truncation_poison_invalid_utf8_json_and_surrogates_fail_without_payload_echo()
 {
    for bytes in [
        vec![0, 0, 0, 0],
        vec![0, 128, 0, 1],
        vec![0, 0],
        vec![0, 0, 0, 8, b'{'],
        b"poisoned-stdout".to_vec(),
        vec![0, 0, 0, 2, 0xff, 0xfe],
    ] {
        let (mut send, receive) = duplex(32);
        send.write_all(&bytes).await.unwrap();
        drop(send);
        assert!(FrameReader::new(receive).read().await.is_err());
    }
    for bytes in [
        b"synthetic-private-marker".as_slice(),
        br#"{"jsonrpc":"2.0","id":"x","result":"\uD800"}"#,
        br#"{"jsonrpc":"2.0","id":"x","result":null} false"#,
    ] {
        let error = decode(bytes).unwrap_err();
        assert!(!error.to_string().contains("synthetic-private-marker"));
    }
    let oversized = json!({"jsonrpc":"2.0","id":"x","result":"x".repeat(MAX_FRAME_BYTES)});
    assert!(encode(&oversized).is_err());
    let mut deep = Value::Null;
    for _ in 0..65 {
        deep = json!([deep]);
    }
    assert!(validate(&json!({"jsonrpc":"2.0","id":"x","result":deep})).is_err());
}
#[tokio::test]
async fn codec_writer_flushes_complete_frames_and_close_reports_eof() {
    let (send, receive) = duplex(64);
    let value = json!({"jsonrpc":"2.0","id":"one","result":{"text":"é😀"}});
    let expected = value.clone();
    let task = tokio::spawn(async move {
        let mut writer = FrameWriter::new(send);
        writer.write(&value).await.unwrap();
        writer.close().await.unwrap();
    });
    let mut reader = FrameReader::new(receive);
    assert_eq!(reader.read().await.unwrap(), Some(expected));
    assert!(reader.read().await.unwrap().is_none());
    task.await.unwrap();
}

#[test]
fn package_source_and_metadata_cancel_responses_roundtrip_through_shared_types() {
    use kode_database::{bridge::wire::CancelStatus, model::MetadataSourceResult};
    let fixtures: Value = serde_json::from_slice(include_bytes!(
        "../../../protocol/jdbc-v1/operation-fixtures.json"
    ))
    .unwrap();
    let source: MetadataSourceResult =
        serde_json::from_value(fixtures["responses"]["metadata.source"].clone()).unwrap();
    assert_eq!(source.context_revision, 7);
    assert_eq!(source.documents.len(), 1);
    assert!(source.documents[0].text.contains("é界😀"));
    assert_eq!(
        serde_json::to_value(source).unwrap(),
        fixtures["responses"]["metadata.source"]
    );
    let cancel: CancelStatus =
        serde_json::from_value(fixtures["responses"]["metadata.cancel"]["status"].clone()).unwrap();
    assert_eq!(cancel, CancelStatus::Requested);
}
