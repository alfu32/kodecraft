use kode_platform::{
    images::{
        DecodedImage,
        cells::{CellMode, CellOptions, render_cells},
        decode::{decode_bytes, load_image},
    },
    jobs::CancellationToken,
};
use std::path::Path;
fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/images")
}
#[test]
fn each_source_decoder_and_dds_subformat_produces_real_bounded_pixels() {
    let token = CancellationToken::new();
    for name in [
        "checker.png",
        "checker.bmp",
        "checker.tga",
        "red.gif",
        "checker.psd",
        "checker-rle.psd",
        "dxt1.dds",
        "dxt3.dds",
        "dxt4.dds",
        "dxt5.dds",
        "checker.kra",
        "checker.svg",
    ] {
        let image = load_image(&fixtures().join(name), &token)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(image.pixel(0, 0)[..3], [255, 0, 0], "{name}");
        assert!(image.width() > 0 && image.height() > 0);
        assert_eq!(
            image.rgba().len(),
            image.width() as usize * image.height() as usize * 4
        );
    }
    let png = load_image(&fixtures().join("checker.png"), &token).unwrap();
    assert_eq!(png.pixel(1, 1), [255, 255, 255, 0]);
}
#[test]
fn malformed_oversized_cancelled_and_unsupported_images_remain_typed_errors() {
    let token = CancellationToken::new();
    for bytes in [b"".as_slice(), b"8BPS", b"DDS ", b"<svg", b"PK\x03\x04"] {
        for extension in ["png", "psd", "dds", "svg", "kra"] {
            assert!(decode_bytes(bytes, extension, &token).is_err());
        }
    }
    let bytes = std::fs::read(fixtures().join("checker.psd")).unwrap();
    for len in 0..bytes.len() {
        assert!(decode_bytes(&bytes[..len], "psd", &token).is_err());
    }
    let mut unsupported = bytes.clone();
    unsupported[22..24].copy_from_slice(&16u16.to_be_bytes());
    assert!(decode_bytes(&unsupported, "psd", &token).is_err());
    let mut enormous = bytes.clone();
    enormous[14..18].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(decode_bytes(&enormous, "psd", &token).is_err());
    token.cancel();
    assert!(matches!(
        decode_bytes(&bytes, "psd", &token),
        Err(kode_core::DomainError::Cancelled)
    ));
    assert!(DecodedImage::new(0, 1, vec![]).is_err());
    assert!(DecodedImage::new(65535, 65535, vec![]).is_err());
    assert!(DecodedImage::new(1, 1, vec![0; 3]).is_err());
}
#[test]
fn cell_jobs_clip_tall_aspects_bound_surfaces_and_acknowledge_cancellation() {
    let image = DecodedImage::new(1, 100, vec![255; 400]).unwrap();
    let options = CellOptions {
        width: 200,
        height: 5,
        mode: CellMode::Quart,
        gray: 0.2,
        scatter: 1600.0,
    };
    let cells = render_cells(&image, options, 10000, || false).unwrap();
    assert_eq!(cells.cells.len(), 1000);
    assert!(matches!(
        render_cells(&image, options, 10000, || true),
        Err(kode_core::DomainError::Cancelled)
    ));
    assert!(
        render_cells(
            &image,
            CellOptions {
                width: 65535,
                height: 65535,
                ..options
            },
            1,
            || false
        )
        .is_err()
    );
    assert!(
        render_cells(
            &image,
            CellOptions {
                gray: f64::NAN,
                ..options
            },
            1,
            || false
        )
        .is_err()
    );
    let white = DecodedImage::new(1, 1, vec![255; 4]).unwrap();
    let cells = render_cells(
        &white,
        CellOptions {
            width: 1,
            height: 1,
            mode: CellMode::Braille,
            gray: 1.0,
            scatter: 1600.0,
        },
        1,
        || false,
    )
    .unwrap();
    assert_eq!(cells.cells[0].glyph, '⠀');
}
#[test]
fn colors_glyphs_sampling_thresholds_and_all_quadrants_match_frozen_kotlin_oracle() {
    let path = fixtures().join("cells-oracle.json");
    let oracle: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for case in oracle["cases"].as_array().unwrap() {
        let pixels: Vec<_> = case["pixels"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|pixel| {
                let argb = pixel.as_u64().unwrap() as u32;
                [
                    (argb >> 16) as u8,
                    (argb >> 8) as u8,
                    argb as u8,
                    (argb >> 24) as u8,
                ]
            })
            .collect();
        let image = DecodedImage::new(
            case["width"].as_u64().unwrap() as u32,
            case["height"].as_u64().unwrap() as u32,
            pixels,
        )
        .unwrap();
        let mode = match case["mode"].as_str().unwrap() {
            "Quart" => CellMode::Quart,
            "Demi" => CellMode::Demi,
            "Bixel" => CellMode::Bixel,
            "Braille" => CellMode::Braille,
            _ => CellMode::Ascii,
        };
        let options = CellOptions {
            width: case["outWidth"].as_u64().unwrap() as u16,
            height: case["outHeight"].as_u64().unwrap() as u16,
            mode,
            gray: case["gray"].as_f64().unwrap(),
            scatter: case["scatter"].as_f64().unwrap(),
        };
        let actual = render_cells(&image, options, options.height.into(), || false).unwrap();
        let expected = case["cells"].as_array().unwrap();
        assert_eq!(actual.cells.len(), expected.len());
        for (actual, expected) in actual.cells.iter().zip(expected) {
            assert_eq!(
                actual.glyph.to_string(),
                expected["glyph"].as_str().unwrap(),
                "{} {:?}",
                case["name"],
                mode
            );
            for (color, key) in [
                (actual.foreground, "foreground"),
                (actual.background, "background"),
            ] {
                assert_eq!(
                    color.to_vec(),
                    expected[key]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_u64().unwrap() as u8)
                        .collect::<Vec<_>>(),
                    "{} {:?} {key}",
                    case["name"],
                    mode
                );
            }
        }
    }
}

#[test]
fn additional_catalog_jpeg_and_webp_decoders_are_qualified_with_encoded_pixels() {
    let rgb = image::RgbImage::from_pixel(4, 4, image::Rgb([255, 0, 0]));
    for (format, extension) in [
        (image::ImageFormat::Jpeg, "jpg"),
        (image::ImageFormat::WebP, "webp"),
    ] {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(rgb.clone())
            .write_to(&mut bytes, format)
            .unwrap();
        let image = decode_bytes(bytes.get_ref(), extension, &CancellationToken::new()).unwrap();
        assert_eq!((image.width(), image.height()), (4, 4));
        let pixel = image.pixel(0, 0);
        assert!(
            pixel[0] > 240 && pixel[1] < 15 && pixel[2] < 15,
            "{extension}: {pixel:?}"
        );
    }
}

#[test]
fn dxt4_unpremultiplies_grayscale_psd_is_exact_and_unavailable_svg_resources_are_visible() {
    let token = CancellationToken::new();
    let mut dds = std::fs::read(fixtures().join("dxt4.dds")).unwrap();
    dds[128] = 128;
    dds[136..138].copy_from_slice(&0x8000u16.to_le_bytes());
    let image = decode_bytes(&dds, "dds", &token).unwrap();
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 128]);
    let mut psd = std::fs::read(fixtures().join("checker.psd")).unwrap()[..40].to_vec();
    psd[12..14].copy_from_slice(&1u16.to_be_bytes());
    psd[24..26].copy_from_slice(&1u16.to_be_bytes());
    psd.extend_from_slice(&[10, 64, 128, 255]);
    let image = decode_bytes(&psd, "psd", &token).unwrap();
    assert_eq!(image.pixel(1, 0), [64, 64, 64, 255]);
    let svg=br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><image href="missing-external.png" width="1" height="1"/></svg>"#;
    assert!(
        decode_bytes(svg, "svg", &token)
            .unwrap_err()
            .to_string()
            .contains("reference")
    );
    assert!(decode_bytes(&[0x1f, 0x8b, 0], "svgz", &token).is_err());
    let mut kra = std::fs::read(fixtures().join("checker.kra")).unwrap();
    let end = kra
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .unwrap();
    kra[end + 10..end + 12].copy_from_slice(&5000u16.to_le_bytes());
    assert!(
        decode_bytes(&kra, "kra", &token)
            .unwrap_err()
            .to_string()
            .contains("too many entries")
    );
}
