use kode_core::{DocumentId, DocumentOrigin, LineIndex, Selection, document::Document};
use kode_tui::editor::{
    layout::VisualLayout,
    render::{EditorGeometry, EditorHeader, RenderOptions, RenderState, render_editor},
};
use kode_tui::theme::Theme;
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const LAYOUT_SAMPLES: usize = 3;
const RENDER_SAMPLES: usize = 31;
const VIEWPORTS: [(u16, u16); 3] = [(80, 24), (120, 40), (20, 6)];

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn cpu_model() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string("/proc/cpuinfo")
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("model name\t: ").map(str::to_owned))
    }
    #[cfg(not(target_os = "linux"))]
    None
}

fn fixture(name: &str, text: &'static str) -> (String, &'static str, Document) {
    let id = DocumentId::fresh().expect("document identity");
    let document = Document::new(
        id,
        DocumentOrigin::Virtual {
            owner: "e13-performance".into(),
            key: name.into(),
            label: name.into(),
        },
        Some("text".into()),
        text,
    );
    (name.into(), text, document)
}

#[test]
#[ignore = "explicit E13 fixture-corpus performance qualification"]
fn records_editor_fixture_layout_and_render_performance() {
    let fixtures = [
        fixture(
            "empty",
            include_str!("../../../../fixtures/editor/empty.txt"),
        ),
        fixture("bom", include_str!("../../../../fixtures/editor/bom.txt")),
        fixture("crlf", include_str!("../../../../fixtures/editor/crlf.txt")),
        fixture("lf", include_str!("../../../../fixtures/editor/lf.txt")),
        fixture(
            "long-line-100k",
            include_str!("../../../../fixtures/editor/long-line.txt"),
        ),
        fixture(
            "many-lines-100k",
            include_str!("../../../../fixtures/editor/many-lines.txt"),
        ),
        fixture(
            "no-final-newline",
            include_str!("../../../../fixtures/editor/no-final-newline.txt"),
        ),
        fixture(
            "unicode",
            include_str!("../../../../fixtures/editor/unicode.txt"),
        ),
    ];
    let theme = Theme::compiled();
    let state = RenderState {
        selection: Selection::default(),
        secondary: Vec::new(),
        scroll_rows: 0,
        focused: false,
    };
    let mut cases = Vec::new();

    for (name, source, document) in fixtures {
        let snapshot = document.snapshot();
        let header = EditorHeader::for_document(&document, None, String::new());
        for (width, height) in VIEWPORTS {
            let area = Rect::new(0, 0, width, height);
            let geometry = EditorGeometry::new(area, snapshot.len_lines(), 0);
            let mut layout_samples = Vec::with_capacity(LAYOUT_SAMPLES);
            let mut layout = None;
            for _ in 0..LAYOUT_SAMPLES {
                let started = Instant::now();
                layout = Some(
                    VisualLayout::build(&snapshot, geometry.text.width as usize, &[])
                        .expect("layout fixture"),
                );
                layout_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
            }
            let layout = layout.expect("at least one layout sample");
            let mut render_samples = Vec::with_capacity(RENDER_SAMPLES);
            for _ in 0..RENDER_SAMPLES {
                let mut buffer = Buffer::empty(area);
                let started = Instant::now();
                render_editor(
                    &layout,
                    &mut buffer,
                    &theme,
                    RenderOptions {
                        geometry,
                        header: &header,
                        state: &state,
                        overlays: &[],
                        warning_lines: &[] as &[LineIndex],
                        fold_starts: &[],
                    },
                );
                render_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
            }
            cases.push(json!({
                "fixture": name,
                "sourceBytes": source.len(),
                "sourceLines": source.lines().count().max(1),
                "viewport": { "width": width, "height": height },
                "layoutRows": layout.rows.len(),
                "layoutMedianMs": median(layout_samples),
                "renderMedianMs": median(render_samples),
                "layoutSamples": LAYOUT_SAMPLES,
                "renderSamples": RENDER_SAMPLES,
            }));
        }
    }

    let report = json!({
        "status": "measured",
        "profile": "release",
        "rustc": Command::new("rustc").arg("--version").output().ok().map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned()),
        "architecture": std::env::consts::ARCH,
        "os": std::env::consts::OS,
        "cpuModel": cpu_model(),
        "unixSeconds": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        "method": "median of fresh layout builds and repeated render_editor calls; includes 3 layout and 31 render samples per case",
        "cases": cases,
    });
    if let Some(path) = std::env::var_os("KODE_E13_PERF_REPORT") {
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(path)
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("report directory");
        }
        fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).expect("performance report");
    }
    println!("{}", serde_json::to_string(&report).unwrap());
}
