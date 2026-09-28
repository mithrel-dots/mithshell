use std::path::Path;

use tree_sitter::Language;
use tree_sitter_highlight::HighlightConfiguration;

use super::{HIGHLIGHT_NAMES, extension};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum LanguageKind {
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
    pub(super) fn name(self) -> &'static str {
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

pub(super) fn build_configuration(
    language: LanguageKind,
) -> Result<HighlightConfiguration, String> {
    let (grammar, highlights): (Language, String) = match language {
        LanguageKind::Rust => (
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
        ),
        LanguageKind::C => (
            tree_sitter_c::LANGUAGE.into(),
            tree_sitter_c::HIGHLIGHT_QUERY.into(),
        ),
        LanguageKind::Cpp => (
            tree_sitter_cpp::LANGUAGE.into(),
            tree_sitter_cpp::HIGHLIGHT_QUERY.into(),
        ),
        LanguageKind::Go => (
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::HIGHLIGHTS_QUERY.into(),
        ),
        LanguageKind::Python => (
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY.into(),
        ),
        LanguageKind::JavaScript => (
            tree_sitter_javascript::LANGUAGE.into(),
            format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
            ),
        ),
        LanguageKind::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY.into(),
        ),
        LanguageKind::Tsx => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY.into(),
        ),
        LanguageKind::Bash => (
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY.into(),
        ),
        LanguageKind::Json => (
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY.into(),
        ),
        LanguageKind::Toml => (
            tree_sitter_toml_ng::LANGUAGE.into(),
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into(),
        ),
        LanguageKind::Yaml => (
            tree_sitter_yaml::LANGUAGE.into(),
            tree_sitter_yaml::HIGHLIGHTS_QUERY.into(),
        ),
        LanguageKind::Html => (
            tree_sitter_html::LANGUAGE.into(),
            tree_sitter_html::HIGHLIGHTS_QUERY.into(),
        ),
        LanguageKind::Css => (
            tree_sitter_css::LANGUAGE.into(),
            tree_sitter_css::HIGHLIGHTS_QUERY.into(),
        ),
        LanguageKind::Markdown => (
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into(),
        ),
    };
    let locals = match language {
        LanguageKind::JavaScript => tree_sitter_javascript::LOCALS_QUERY,
        LanguageKind::TypeScript | LanguageKind::Tsx => tree_sitter_typescript::LOCALS_QUERY,
        _ => "",
    };
    let mut configuration =
        HighlightConfiguration::new(grammar, language.name(), &highlights, "", locals).map_err(
            |error| format!("cannot configure {} highlighting: {error}", language.name()),
        )?;
    configuration.configure(HIGHLIGHT_NAMES);
    Ok(configuration)
}

pub(super) fn detect_language(path: &Path, first_line: Option<&str>) -> Option<LanguageKind> {
    let filename = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if matches!(
        filename.as_str(),
        "makefile" | "bashrc" | "zshrc" | "profile"
    ) {
        return Some(LanguageKind::Bash);
    }
    let language = match extension(path).as_str() {
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
        } else if line.contains("sh") {
            Some(LanguageKind::Bash)
        } else {
            None
        }
    })
}
