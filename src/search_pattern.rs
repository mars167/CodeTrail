use anyhow::{anyhow, Result};
use clap::ValueEnum;
use globset::{GlobBuilder, GlobMatcher};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum SearchPatternMode {
    Literal,
    Regex,
    Wildcard,
    Glob,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ContentPatternMode {
    Literal,
    Regex,
    Wildcard,
}

impl ContentPatternMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Regex => "regex",
            Self::Wildcard => "wildcard",
        }
    }
}

impl From<ContentPatternMode> for SearchPatternMode {
    fn from(value: ContentPatternMode) -> Self {
        match value {
            ContentPatternMode::Literal => SearchPatternMode::Literal,
            ContentPatternMode::Regex => SearchPatternMode::Regex,
            ContentPatternMode::Wildcard => SearchPatternMode::Wildcard,
        }
    }
}

impl SearchPatternMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "literal" | "path_substring" => Ok(Self::Literal),
            "regex" => Ok(Self::Regex),
            "wildcard" => Ok(Self::Wildcard),
            "glob" | "strict_glob" => Ok(Self::Glob),
            other => Err(anyhow!("unsupported search mode: {other}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Regex => "regex",
            Self::Wildcard => "wildcard",
            Self::Glob => "glob",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternTarget {
    Content,
    Path,
}

#[derive(Clone, Debug)]
pub struct PatternMatcher {
    mode: SearchPatternMode,
    case_sensitive: bool,
    regex: Option<Regex>,
    glob: Option<GlobMatcher>,
    literal: String,
}

impl PatternMatcher {
    pub fn compile(
        pattern: &str,
        mode: SearchPatternMode,
        case_sensitive: bool,
        target: PatternTarget,
    ) -> Result<Self> {
        let regex = match mode {
            SearchPatternMode::Literal if target == PatternTarget::Content => Some(
                RegexBuilder::new(&regex::escape(pattern))
                    .case_insensitive(!case_sensitive)
                    .build()?,
            ),
            SearchPatternMode::Literal | SearchPatternMode::Glob => None,
            SearchPatternMode::Regex => Some(
                RegexBuilder::new(pattern)
                    .case_insensitive(!case_sensitive)
                    .build()?,
            ),
            SearchPatternMode::Wildcard => Some(
                RegexBuilder::new(&wildcard_regex(pattern, target))
                    .case_insensitive(!case_sensitive)
                    .build()?,
            ),
        };
        let glob = if mode == SearchPatternMode::Glob {
            Some(
                GlobBuilder::new(pattern)
                    .case_insensitive(!case_sensitive)
                    .literal_separator(false)
                    .build()?
                    .compile_matcher(),
            )
        } else {
            None
        };
        Ok(Self {
            mode,
            case_sensitive,
            regex,
            glob,
            literal: pattern.to_string(),
        })
    }

    pub fn is_match(&self, candidate: &str) -> bool {
        match self.mode {
            SearchPatternMode::Literal => {
                literal_contains(candidate, &self.literal, self.case_sensitive)
            }
            SearchPatternMode::Regex | SearchPatternMode::Wildcard => self
                .regex
                .as_ref()
                .is_some_and(|regex| regex.is_match(candidate)),
            SearchPatternMode::Glob => self
                .glob
                .as_ref()
                .is_some_and(|glob| glob.is_match(candidate)),
        }
    }

    pub fn mode(&self) -> SearchPatternMode {
        self.mode
    }

    pub fn regex(&self) -> Option<&Regex> {
        self.regex.as_ref()
    }
}

pub fn compile_any(
    patterns: &[String],
    mode: SearchPatternMode,
    case_sensitive: bool,
    target: PatternTarget,
) -> Result<Vec<PatternMatcher>> {
    patterns
        .iter()
        .map(|pattern| PatternMatcher::compile(pattern, mode, case_sensitive, target))
        .collect()
}

pub fn normalize_extension(value: &str) -> String {
    value.trim_start_matches('.').to_ascii_lowercase()
}

fn literal_contains(candidate: &str, needle: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        candidate.contains(needle)
    } else {
        candidate.to_lowercase().contains(&needle.to_lowercase())
    }
}

fn wildcard_regex(pattern: &str, target: PatternTarget) -> String {
    let mut regex = String::new();
    for ch in pattern.chars() {
        match ch {
            '*' => match target {
                PatternTarget::Content => regex.push_str("[^\\n]*"),
                PatternTarget::Path => regex.push_str(".*"),
            },
            '?' => match target {
                PatternTarget::Content => regex.push_str("[^\\n]"),
                PatternTarget::Path => regex.push('.'),
            },
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
    }
    regex
}
