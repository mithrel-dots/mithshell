use std::{
    collections::HashMap,
    fs::{self, File},
    hash::{DefaultHasher, Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, UNIX_EPOCH},
};

use async_channel::{Receiver, Sender};
use image::ImageReader;
use serde::Deserialize;
use tree_sitter::Language;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};
use wait_timeout::ChildExt;

const MAX_TEXT_BYTES: usize = 512 * 1024;
const MAX_TEXT_LINES: usize = 5_000;
const PROCESS_TIMEOUT: Duration = Duration::from_secs(5);

pub const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "conditional",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "exception",
    "function",
    "function.builtin",
    "function.call",
    "function.method",
    "keyword",
    "label",
    "module",
    "namespace",
    "number",
    "operator",
    "property",
    "punctuation",
    "string",
    "string.escape",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

#[derive(Debug, Clone)]
pub struct PreviewRequest {
    pub monitor: String,
    pub generation: u64,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PreviewEvent {
    pub monitor: String,
    pub generation: u64,
    pub result: Result<PreviewData, String>,
}

#[derive(Debug, Clone)]
pub struct PreviewData {
    pub content: PreviewContent,
    pub metadata: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub enum PreviewContent {
    Text {
        text: String,
        highlights: Vec<HighlightSpan>,
    },
    Image(PathBuf),
    VideoThumbnail(PathBuf),
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: i32,
    pub end: i32,
    pub style: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LanguageKind {
    Rust,
    C,
    Cpp,
    Go,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Bash,
    Json,
    Toml,
    Yaml,
    Html,
    Css,
    Markdown,
}

impl LanguageKind {
    fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::Go => "Go",
            Self::Python => "Python",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Tsx => "TSX",
            Self::Bash => "Bash",
            Self::Json => "JSON",
            Self::Toml => "TOML",
            Self::Yaml => "YAML",
            Self::Html => "HTML",
            Self::Css => "CSS",
            Self::Markdown => "Markdown",
        }
    }
}

struct PreviewEngine {
    highlighter: Highlighter,
    configurations: HashMap<LanguageKind, HighlightConfiguration>,
}

impl PreviewEngine {
    fn new() -> Self {
        Self {
            highlighter: Highlighter::new(),
            configurations: HashMap::new(),
        }
    }

    fn load(&mut self, path: &Path) -> Result<PreviewData, String> {
        let path = path
            .canonicalize()
            .map_err(|error| format!("cannot resolve preview: {error}"))?;
        let file_metadata = path
            .metadata()
            .map_err(|error| format!("cannot inspect preview: {error}"))?;
        if !file_metadata.is_file() {
            return Err("preview path is not a regular file".into());
        }

        if is_image_path(&path) {
            return load_image(&path, &file_metadata);
        }
        if is_video_path(&path) {
            return load_video(&path, &file_metadata);
        }
        if let Some(language) = detect_language(&path, None) {
            return self.load_text(&path, &file_metadata, Some(language));
        }

        let mut probe = [0_u8; 8 * 1024];
        let count = File::open(&path)
            .and_then(|mut file| file.read(&mut probe))
            .map_err(|error| format!("cannot read preview: {error}"))?;
        if count == 0
            || (!probe[..count].contains(&0) && std::str::from_utf8(&probe[..count]).is_ok())
        {
            return self.load_text(&path, &file_metadata, None);
        }

        Ok(PreviewData {
            content: PreviewContent::Generic,
            metadata: common_metadata(&path, &file_metadata, "Binary"),
        })
    }

    fn load_text(
        &mut self,
        path: &Path,
        metadata: &fs::Metadata,
        language_hint: Option<LanguageKind>,
    ) -> Result<PreviewData, String> {
        let mut bytes = Vec::with_capacity(MAX_TEXT_BYTES.min(metadata.len() as usize));
        File::open(path)
            .map_err(|error| format!("cannot open text preview: {error}"))?
            .take((MAX_TEXT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read text preview: {error}"))?;

        let truncated_bytes = bytes.len() > MAX_TEXT_BYTES;
        bytes.truncate(MAX_TEXT_BYTES);
        while std::str::from_utf8(&bytes).is_err() && bytes.pop().is_some() {}
        let mut text = String::from_utf8(bytes).map_err(|_| "text preview is not UTF-8")?;
        let language = language_hint.or_else(|| detect_language(path, text.lines().next()));

        let mut line_count = text.lines().count();
        let mut truncated_lines = false;
        if line_count > MAX_TEXT_LINES {
            let end = text
                .match_indices('\n')
                .nth(MAX_TEXT_LINES - 1)
                .map_or(text.len(), |(index, _)| index + 1);
            text.truncate(end);
            line_count = MAX_TEXT_LINES;
            truncated_lines = true;
        }
        let truncated = truncated_bytes || truncated_lines || metadata.len() > text.len() as u64;
        let highlights = language
            .map(|language| self.highlight(language, &text))
            .transpose()?
            .unwrap_or_default();

        let mut fields = common_metadata(path, metadata, "Text");
        fields.push((
            "Language".into(),
            language.map_or("Plain Text", LanguageKind::name).into(),
        ));
        fields.push((
            "Lines".into(),
            if truncated {
                format!("{line_count}+")
            } else {
                line_count.to_string()
            },
        ));
        fields.push(("Encoding".into(), "UTF-8".into()));
        if truncated {
            fields.push((
                "Preview".into(),
                format!(
                    "truncated at {} KiB / {MAX_TEXT_LINES} lines",
                    MAX_TEXT_BYTES / 1024
                ),
            ));
        }
        Ok(PreviewData {
            content: PreviewContent::Text { text, highlights },
            metadata: fields,
        })
    }

    fn highlight(
        &mut self,
        language: LanguageKind,
        source: &str,
    ) -> Result<Vec<HighlightSpan>, String> {
        if let std::collections::hash_map::Entry::Vacant(entry) =
            self.configurations.entry(language)
        {
            entry.insert(build_configuration(language)?);
        }
        let configuration = self.configurations.get(&language).unwrap();
        let events = self
            .highlighter
            .highlight(configuration, source.as_bytes(), None, |_| None)
            .map_err(|error| format!("Tree-sitter highlight failed: {error}"))?;
        let mut raw = Vec::new();
        let mut stack = Vec::new();
        for event in events {
            match event.map_err(|error| format!("Tree-sitter highlight failed: {error}"))? {
                HighlightEvent::HighlightStart(highlight) => stack.push(highlight.0),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    if let Some(style) = stack.last().copied()
                        && start < end
                    {
                        raw.push((start, end, style));
                    }
                }
            }
        }
        Ok(byte_spans_to_char_spans(source, &raw))
    }
}

pub fn start_loader(
    event_sender: Sender<PreviewEvent>,
) -> (Sender<PreviewRequest>, thread::JoinHandle<()>) {
    let (request_sender, request_receiver) = async_channel::unbounded();
    let handle = thread::spawn(move || run_loader(request_receiver, event_sender));
    (request_sender, handle)
}

fn run_loader(receiver: Receiver<PreviewRequest>, sender: Sender<PreviewEvent>) {
    let mut engine = PreviewEngine::new();
    while let Ok(request) = receiver.recv_blocking() {
        let pending = coalesce_requests(request, std::iter::from_fn(|| receiver.try_recv().ok()));
        for request in pending {
            let result = engine.load(&request.path);
            let _ = sender.send_blocking(PreviewEvent {
                monitor: request.monitor,
                generation: request.generation,
                result,
            });
        }
    }
}

/// Keeps only the newest queued request per monitor.
///
/// Arrowing through results queues one request per row. Loading each in turn
/// can mean serial ffprobe and ffmpegthumbnailer runs whose output is thrown
/// away on arrival because the selection already moved on.
fn coalesce_requests(
    first: PreviewRequest,
    rest: impl Iterator<Item = PreviewRequest>,
) -> Vec<PreviewRequest> {
    let mut pending: HashMap<String, PreviewRequest> = HashMap::new();
    pending.insert(first.monitor.clone(), first);
    for request in rest {
        pending.insert(request.monitor.clone(), request);
    }
    pending.into_values().collect()
}

fn build_configuration(language: LanguageKind) -> Result<HighlightConfiguration, String> {
    let (grammar, highlights, injections, locals): (Language, String, &str, &str) = match language {
        LanguageKind::Rust => (
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
            "",
            "",
        ),
        LanguageKind::C => (
            tree_sitter_c::LANGUAGE.into(),
            tree_sitter_c::HIGHLIGHT_QUERY.into(),
            "",
            "",
        ),
        LanguageKind::Cpp => (
            tree_sitter_cpp::LANGUAGE.into(),
            tree_sitter_cpp::HIGHLIGHT_QUERY.into(),
            "",
            "",
        ),
        LanguageKind::Go => (
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::HIGHLIGHTS_QUERY.into(),
            "",
            "",
        ),
        LanguageKind::Python => (
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY.into(),
            "",
            "",
        ),
        LanguageKind::JavaScript => (
            tree_sitter_javascript::LANGUAGE.into(),
            format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
            ),
            "",
            tree_sitter_javascript::LOCALS_QUERY,
        ),
        LanguageKind::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY.into(),
            "",
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        LanguageKind::Tsx => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY.into(),
            "",
            tree_sitter_typescript::LOCALS_QUERY,
        ),
        LanguageKind::Bash => (
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY.into(),
            "",
            "",
        ),
        LanguageKind::Json => (
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY.into(),
            "",
            "",
        ),
        LanguageKind::Toml => (
            tree_sitter_toml_ng::LANGUAGE.into(),
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into(),
            "",
            "",
        ),
        LanguageKind::Yaml => (
            tree_sitter_yaml::LANGUAGE.into(),
            tree_sitter_yaml::HIGHLIGHTS_QUERY.into(),
            "",
            "",
        ),
        LanguageKind::Html => (
            tree_sitter_html::LANGUAGE.into(),
            tree_sitter_html::HIGHLIGHTS_QUERY.into(),
            "",
            "",
        ),
        LanguageKind::Css => (
            tree_sitter_css::LANGUAGE.into(),
            tree_sitter_css::HIGHLIGHTS_QUERY.into(),
            "",
            "",
        ),
        LanguageKind::Markdown => (
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into(),
            "",
            "",
        ),
    };
    let mut configuration =
        HighlightConfiguration::new(grammar, language.name(), &highlights, injections, locals)
            .map_err(|error| {
                format!("cannot configure {} highlighting: {error}", language.name())
            })?;
    configuration.configure(HIGHLIGHT_NAMES);
    Ok(configuration)
}

fn detect_language(path: &Path, first_line: Option<&str>) -> Option<LanguageKind> {
    let filename = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if matches!(
        filename.as_str(),
        "makefile" | "bashrc" | "zshrc" | "profile"
    ) {
        return Some(LanguageKind::Bash);
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let language = match extension.as_str() {
        "rs" => Some(LanguageKind::Rust),
        "c" | "h" => Some(LanguageKind::C),
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => Some(LanguageKind::Cpp),
        "go" => Some(LanguageKind::Go),
        "py" | "pyw" => Some(LanguageKind::Python),
        "js" | "jsx" | "mjs" | "cjs" => Some(LanguageKind::JavaScript),
        "ts" | "mts" | "cts" => Some(LanguageKind::TypeScript),
        "tsx" => Some(LanguageKind::Tsx),
        "sh" | "bash" | "zsh" | "fish" => Some(LanguageKind::Bash),
        "json" | "jsonc" => Some(LanguageKind::Json),
        "toml" => Some(LanguageKind::Toml),
        "yaml" | "yml" => Some(LanguageKind::Yaml),
        "html" | "htm" => Some(LanguageKind::Html),
        "css" => Some(LanguageKind::Css),
        "md" | "markdown" | "mdown" => Some(LanguageKind::Markdown),
        _ => None,
    };
    language.or_else(|| {
        let line = first_line?.to_ascii_lowercase();
        if !line.starts_with("#!") {
            return None;
        }
        if line.contains("python") {
            Some(LanguageKind::Python)
        } else if line.contains("bash") || line.contains("sh") || line.contains("zsh") {
            Some(LanguageKind::Bash)
        } else {
            None
        }
    })
}

fn load_image(path: &Path, metadata: &fs::Metadata) -> Result<PreviewData, String> {
    if extension(path) == "svg" {
        let mut source = String::new();
        File::open(path)
            .map_err(|error| format!("cannot open SVG preview: {error}"))?
            .take(128 * 1024)
            .read_to_string(&mut source)
            .map_err(|error| format!("cannot read SVG preview: {error}"))?;
        let mut fields = common_metadata(path, metadata, "Image");
        if let Some((width, height)) = svg_dimensions(&source) {
            fields.push(("Resolution".into(), format!("{width} x {height}")));
        }
        fields.push(("Format".into(), "SVG".into()));
        return Ok(PreviewData {
            content: PreviewContent::Image(path.to_owned()),
            metadata: fields,
        });
    }
    let reader = ImageReader::open(path)
        .map_err(|error| format!("cannot open image preview: {error}"))?
        .with_guessed_format()
        .map_err(|error| format!("cannot detect image format: {error}"))?;
    let format = reader.format();
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| format!("cannot read image dimensions: {error}"))?;
    let mut fields = common_metadata(path, metadata, "Image");
    fields.push(("Resolution".into(), format!("{width} x {height}")));
    if let Some(format) = format {
        fields.push(("Format".into(), format!("{format:?}").to_uppercase()));
    }
    Ok(PreviewData {
        content: PreviewContent::Image(path.to_owned()),
        metadata: fields,
    })
}

fn svg_dimensions(source: &str) -> Option<(SvgNumber, SvgNumber)> {
    let width = svg_attribute(source, "width");
    let height = svg_attribute(source, "height");
    if let (Some(width), Some(height)) = (width, height) {
        return Some((SvgNumber(width), SvgNumber(height)));
    }
    let view_box = svg_attribute_text(source, "viewBox")?;
    let values: Vec<_> = view_box
        .split(|character: char| character.is_ascii_whitespace() || character == ',')
        .filter_map(|value| value.parse::<f64>().ok())
        .collect();
    (values.len() == 4).then(|| (SvgNumber(values[2]), SvgNumber(values[3])))
}

fn svg_attribute(source: &str, name: &str) -> Option<f64> {
    let value = svg_attribute_text(source, name)?;
    let number: String = value
        .chars()
        .take_while(|character| character.is_ascii_digit() || matches!(character, '.' | '-' | '+'))
        .collect();
    number.parse().ok()
}

fn svg_attribute_text<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let start = source.find("<svg")?;
    let element = &source[start..source[start..].find('>')? + start];
    let marker = format!("{name}=");
    let value = &element[element.find(&marker)? + marker.len()..];
    let quote = value.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let value = &value[quote.len_utf8()..];
    Some(&value[..value.find(quote)?])
}

struct SvgNumber(f64);

impl std::fmt::Display for SvgNumber {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.fract() == 0.0 {
            write!(formatter, "{:.0}", self.0)
        } else {
            write!(formatter, "{:.2}", self.0)
        }
    }
}

fn load_video(path: &Path, metadata: &fs::Metadata) -> Result<PreviewData, String> {
    let probe = run_ffprobe(path)?;
    let stream = probe
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .ok_or("ffprobe did not report a video stream")?;
    let mut fields = common_metadata(path, metadata, "Video");
    if let (Some(width), Some(height)) = (stream.width, stream.height) {
        fields.push(("Resolution".into(), format!("{width} x {height}")));
    }
    if let Some(codec) = &stream.codec_name {
        fields.push(("Codec".into(), codec.to_uppercase()));
    }
    if let Some(pixel_format) = &stream.pix_fmt {
        fields.push(("Pixel format".into(), pixel_format.clone()));
    }
    if let Some(rate) = stream
        .avg_frame_rate
        .as_deref()
        .and_then(parse_frame_rate)
        .or_else(|| stream.r_frame_rate.as_deref().and_then(parse_frame_rate))
    {
        fields.push(("Frame rate".into(), format!("{rate:.3} fps")));
    }
    if let Some(frames) = &stream.nb_frames {
        fields.push(("Frames".into(), frames.clone()));
    }
    if let Some(duration) = probe
        .format
        .as_ref()
        .and_then(|format| format.duration.as_deref())
        .and_then(|duration| duration.parse::<f64>().ok())
    {
        fields.push(("Duration".into(), format_duration(duration)));
    }
    if let Some(bit_rate) = probe
        .format
        .as_ref()
        .and_then(|format| format.bit_rate.as_deref())
        .and_then(|bit_rate| bit_rate.parse::<u64>().ok())
    {
        fields.push((
            "Bit rate".into(),
            format!("{:.1} Mbit/s", bit_rate as f64 / 1_000_000.0),
        ));
    }
    let thumbnail = video_thumbnail(path, metadata)?;
    Ok(PreviewData {
        content: PreviewContent::VideoThumbnail(thumbnail),
        metadata: fields,
    })
}

fn run_ffprobe(path: &Path) -> Result<FfprobeOutput, String> {
    let mut child = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start ffprobe: {error}"))?;
    let mut stdout = child.stdout.take().ok_or("ffprobe stdout is unavailable")?;
    let mut stderr = child.stderr.take().ok_or("ffprobe stderr is unavailable")?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });
    let status = child
        .wait_timeout(PROCESS_TIMEOUT)
        .map_err(|error| format!("cannot wait for ffprobe: {error}"))?;
    let status = if let Some(status) = status {
        status
    } else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("ffprobe timed out".into());
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "ffprobe stdout reader panicked")?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "ffprobe stderr reader panicked")?;
    if !status.success() {
        return Err(String::from_utf8_lossy(&stderr).trim().to_owned());
    }
    serde_json::from_slice(&stdout).map_err(|error| format!("invalid ffprobe JSON: {error}"))
}

fn video_thumbnail(path: &Path, metadata: &fs::Metadata) -> Result<PathBuf, String> {
    let cache = preview_cache_dir();
    fs::create_dir_all(&cache).map_err(|error| format!("cannot create preview cache: {error}"))?;
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|modified| modified.as_nanos())
        .hash(&mut hasher);
    let output = cache.join(format!("{:016x}.png", hasher.finish()));
    if output.is_file() {
        return Ok(output);
    }
    let temporary = output.with_extension(format!("{}.tmp.png", std::process::id()));
    let mut child = Command::new("ffmpegthumbnailer")
        .arg("-i")
        .arg(path)
        .arg("-o")
        .arg(&temporary)
        .args(["-s", "512", "-t", "10", "-q", "8", "-c", "png"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot start ffmpegthumbnailer: {error}"))?;
    let status = child
        .wait_timeout(PROCESS_TIMEOUT)
        .map_err(|error| format!("cannot wait for ffmpegthumbnailer: {error}"))?;
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(&temporary);
        return Err("video thumbnail timed out".into());
    };
    if !status.success() || !temporary.is_file() {
        let _ = fs::remove_file(&temporary);
        return Err("ffmpegthumbnailer could not generate a preview".into());
    }
    fs::rename(&temporary, &output)
        .map_err(|error| format!("cannot store video thumbnail: {error}"))?;
    Ok(output)
}

fn common_metadata(path: &Path, metadata: &fs::Metadata, kind: &str) -> Vec<(String, String)> {
    vec![
        (
            "File".into(),
            path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into(),
            ),
        ),
        ("Type".into(), kind.into()),
        ("Size".into(), human_size(metadata.len())),
    ]
}

fn preview_cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("mithshell/previews")
}

fn is_image_path(path: &Path) -> bool {
    matches!(
        extension(path).as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tiff" | "tif" | "svg" | "avif"
    )
}

fn is_video_path(path: &Path) -> bool {
    matches!(
        extension(path).as_str(),
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "wmv" | "flv" | "mpeg" | "mpg"
    )
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn parse_frame_rate(value: &str) -> Option<f64> {
    if let Some((numerator, denominator)) = value.split_once('/') {
        let numerator = numerator.parse::<f64>().ok()?;
        let denominator = denominator.parse::<f64>().ok()?;
        (denominator != 0.0).then_some(numerator / denominator)
    } else {
        value.parse().ok()
    }
}

fn format_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn byte_spans_to_char_spans(source: &str, spans: &[(usize, usize, usize)]) -> Vec<HighlightSpan> {
    let mut boundaries: Vec<_> = spans
        .iter()
        .flat_map(|(start, end, _)| [*start, *end])
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut offsets = HashMap::with_capacity(boundaries.len());
    let mut chars = source.char_indices().peekable();
    let mut count = 0_i32;
    for boundary in boundaries {
        while chars.peek().is_some_and(|(byte, _)| *byte < boundary) {
            chars.next();
            count += 1;
        }
        offsets.insert(boundary, count);
    }
    spans
        .iter()
        .filter_map(|(start, end, style)| {
            Some(HighlightSpan {
                start: *offsets.get(start)?,
                end: *offsets.get(end)?,
                style: *style,
            })
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    format: Option<FfprobeFormat>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    nb_frames: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
    bit_rate: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(monitor: &str, generation: u64, path: &str) -> PreviewRequest {
        PreviewRequest {
            monitor: monitor.to_owned(),
            generation,
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn only_the_newest_request_per_monitor_is_loaded() {
        let pending = coalesce_requests(
            request("DP-1", 1, "/a"),
            [
                request("DP-1", 2, "/b"),
                request("DP-2", 7, "/c"),
                request("DP-1", 3, "/d"),
            ]
            .into_iter(),
        );

        assert_eq!(pending.len(), 2, "one request survives per monitor");
        let dp1 = pending
            .iter()
            .find(|request| request.monitor == "DP-1")
            .unwrap();
        assert_eq!(dp1.generation, 3, "superseded requests are dropped");
        assert_eq!(dp1.path, PathBuf::from("/d"));
        let dp2 = pending
            .iter()
            .find(|request| request.monitor == "DP-2")
            .unwrap();
        assert_eq!(dp2.generation, 7, "other monitors are not discarded");
    }

    #[test]
    fn a_lone_request_is_preserved() {
        let pending = coalesce_requests(request("DP-1", 1, "/a"), std::iter::empty());
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].generation, 1);
    }

    fn temporary_path(extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mithshell-preview-test-{}-{}.{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("worker"),
            extension
        ))
    }

    #[test]
    fn detects_common_languages_and_shebangs() {
        assert_eq!(
            detect_language(Path::new("main.rs"), None),
            Some(LanguageKind::Rust)
        );
        assert_eq!(
            detect_language(Path::new("view.tsx"), None),
            Some(LanguageKind::Tsx)
        );
        assert_eq!(
            detect_language(Path::new("script"), Some("#!/usr/bin/env python3")),
            Some(LanguageKind::Python)
        );
    }

    #[test]
    fn converts_utf8_byte_ranges_to_character_ranges() {
        let spans = byte_spans_to_char_spans("a😀bc", &[(1, 5, 2), (5, 7, 3)]);
        assert_eq!(
            spans,
            [
                HighlightSpan {
                    start: 1,
                    end: 2,
                    style: 2
                },
                HighlightSpan {
                    start: 2,
                    end: 4,
                    style: 3
                }
            ]
        );
    }

    #[test]
    fn parses_frame_rate_and_formats_duration() {
        assert_eq!(parse_frame_rate("30000/1001").unwrap().round(), 30.0);
        assert_eq!(parse_frame_rate("0/0"), None);
        assert_eq!(format_duration(3723.0), "1:02:03");
    }

    #[test]
    fn parses_ffprobe_metadata() {
        let probe: FfprobeOutput = serde_json::from_str(
            r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"60/1"}],"format":{"duration":"12.5","bit_rate":"4000000"}}"#,
        )
        .unwrap();
        assert_eq!(probe.streams[0].width, Some(1920));
        assert_eq!(probe.format.unwrap().duration.as_deref(), Some("12.5"));
    }

    #[test]
    fn tree_sitter_highlights_rust() {
        let mut engine = PreviewEngine::new();
        let spans = engine
            .highlight(LanguageKind::Rust, "fn main() { let value = 1; }")
            .unwrap();
        assert!(!spans.is_empty());
    }

    #[test]
    fn builds_every_bundled_highlight_configuration() {
        for language in [
            LanguageKind::Rust,
            LanguageKind::C,
            LanguageKind::Cpp,
            LanguageKind::Go,
            LanguageKind::Python,
            LanguageKind::JavaScript,
            LanguageKind::TypeScript,
            LanguageKind::Tsx,
            LanguageKind::Bash,
            LanguageKind::Json,
            LanguageKind::Toml,
            LanguageKind::Yaml,
            LanguageKind::Html,
            LanguageKind::Css,
            LanguageKind::Markdown,
        ] {
            build_configuration(language)
                .unwrap_or_else(|error| panic!("{}: {error}", language.name()));
        }
    }

    #[test]
    fn loads_highlighted_text_metadata() {
        let path = temporary_path("rs");
        fs::write(&path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();
        let mut engine = PreviewEngine::new();
        let preview = engine.load(&path).unwrap();
        fs::remove_file(path).unwrap();
        let PreviewContent::Text { text, highlights } = preview.content else {
            panic!("expected text preview");
        };
        assert_eq!(text.lines().count(), 3);
        assert!(!highlights.is_empty());
        assert!(
            preview
                .metadata
                .contains(&("Language".into(), "Rust".into()))
        );
        assert!(preview.metadata.contains(&("Lines".into(), "3".into())));
    }

    #[test]
    fn reads_image_resolution_without_full_ui() {
        let path = temporary_path("png");
        image::RgbImage::new(3, 2).save(&path).unwrap();
        let mut engine = PreviewEngine::new();
        let preview = engine.load(&path).unwrap();
        fs::remove_file(path).unwrap();
        assert!(matches!(preview.content, PreviewContent::Image(_)));
        assert!(
            preview
                .metadata
                .contains(&("Resolution".into(), "3 x 2".into()))
        );
    }

    #[test]
    fn reads_svg_dimensions_from_view_box() {
        let dimensions = svg_dimensions(r#"<svg viewBox="0 0 1280 720"></svg>"#).unwrap();
        assert_eq!(dimensions.0.to_string(), "1280");
        assert_eq!(dimensions.1.to_string(), "720");
    }
}
