use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MAX_COMPATIBLE_VARIANTS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum InputMode {
    Compatible,
    Strict,
}

impl InputMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compatible => "compatible",
            Self::Strict => "strict",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputVariant {
    pub value: String,
    pub kind: &'static str,
}

#[derive(Clone, Debug)]
pub struct InputPlan {
    pub raw: String,
    pub mode: InputMode,
    pub variants: Vec<InputVariant>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolMatchMode {
    Exact,
    Contains,
}

impl InputPlan {
    pub fn new(input: &str, mode: InputMode) -> Self {
        if mode == InputMode::Strict {
            return Self {
                raw: input.to_string(),
                mode,
                variants: vec![InputVariant {
                    value: input.to_string(),
                    kind: "raw",
                }],
                truncated: false,
            };
        }

        let mut plan = Self {
            raw: input.to_string(),
            mode,
            variants: Vec::new(),
            truncated: false,
        };
        plan.push(input.to_string(), "raw");
        let trimmed = input.trim();
        plan.push(trimmed.to_string(), "trimmed");
        let signature_base = remove_signature(trimmed);
        plan.push(signature_base.clone(), "signature_base");
        let tail = qualified_tail(trimmed);
        plan.push(tail.clone(), "qualified_tail");
        plan.push(remove_signature(&tail), "signature_tail");
        for value in style_sources(trimmed, &signature_base, &tail) {
            let key = style_key(&value);
            if !key.is_empty() {
                plan.push(key, "style_key");
            }
        }
        for value in case_fold_sources(trimmed, &signature_base, &tail) {
            let folded = value.to_lowercase();
            if !folded.is_empty() {
                plan.push(folded, "case_fold");
            }
        }
        plan
    }

    pub fn expanded(&self) -> bool {
        if self.mode != InputMode::Compatible {
            return false;
        }
        if self.truncated {
            return true;
        }
        let raw = self.raw.as_str();
        let trimmed = self.raw.trim();
        self.variants.iter().any(|variant| {
            !matches!(variant.kind, "raw" | "trimmed")
                && variant.value != raw
                && variant.value != trimmed
        })
    }

    pub fn expansion_warning(&self) -> Option<String> {
        if !self.expanded() {
            return None;
        }
        let suffix = if self.truncated {
            "; compatible variants were capped"
        } else {
            ""
        };
        Some(format!(
            "query_input_expanded: compatible input generated {} symbol variants{}",
            self.variants.len(),
            suffix
        ))
    }

    pub fn matched_variant(
        &self,
        candidate: &str,
        case_sensitive: bool,
        mode: SymbolMatchMode,
    ) -> Option<&InputVariant> {
        let case_sensitive = case_sensitive || self.mode == InputMode::Strict;
        self.variants
            .iter()
            .find(|variant| variant_matches(candidate, variant, case_sensitive, mode))
    }
}

pub fn matched_variant_value(variant: &InputVariant) -> Value {
    json!({
        "kind": variant.kind,
        "value": variant.value
    })
}

pub fn attach_matched_input(mut value: Value, variant: &InputVariant) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "matchedInputVariant".to_string(),
            matched_variant_value(variant),
        );
    }
    value
}

pub fn compatible_input_needs_expansion(input: &str, mode: InputMode) -> bool {
    if mode == InputMode::Strict {
        return false;
    }
    let trimmed = input.trim();
    trimmed != input
        || remove_signature(trimmed) != trimmed
        || qualified_tail(trimmed) != trimmed
        || trimmed.contains('_')
        || trimmed.contains('-')
}

pub fn style_key(value: &str) -> String {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_is_lower_or_digit = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && previous_is_lower_or_digit && !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            current.push(ch.to_ascii_lowercase());
            previous_is_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            previous_is_lower_or_digit = false;
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens.join("_")
}

fn variant_matches(
    candidate: &str,
    variant: &InputVariant,
    case_sensitive: bool,
    mode: SymbolMatchMode,
) -> bool {
    if variant.kind == "style_key" {
        return compare(&style_key(candidate), &variant.value, false, mode);
    }
    if variant.kind == "case_fold" {
        if case_sensitive {
            return false;
        }
        return compare(&candidate.to_lowercase(), &variant.value, true, mode);
    }
    compare(candidate, &variant.value, case_sensitive, mode)
}

fn compare(candidate: &str, needle: &str, case_sensitive: bool, mode: SymbolMatchMode) -> bool {
    match (case_sensitive, mode) {
        (true, SymbolMatchMode::Exact) => candidate == needle,
        (true, SymbolMatchMode::Contains) => candidate.contains(needle),
        (false, SymbolMatchMode::Exact) => candidate.eq_ignore_ascii_case(needle),
        (false, SymbolMatchMode::Contains) => {
            candidate.to_lowercase().contains(&needle.to_lowercase())
        }
    }
}

fn remove_signature(value: &str) -> String {
    value
        .find('(')
        .map(|idx| value[..idx].trim().to_string())
        .unwrap_or_else(|| value.to_string())
}

fn qualified_tail(value: &str) -> String {
    let trimmed = value.trim();
    let mut start = 0;
    for separator in ['.', ':', '#', '$'] {
        if let Some(idx) = trimmed.rfind(separator) {
            start = start.max(idx + separator.len_utf8());
        }
    }
    trimmed[start..].trim().to_string()
}

fn style_sources(trimmed: &str, signature_base: &str, tail: &str) -> Vec<String> {
    vec![
        trimmed.to_string(),
        signature_base.to_string(),
        tail.to_string(),
        remove_signature(tail),
    ]
}

fn case_fold_sources(trimmed: &str, signature_base: &str, tail: &str) -> Vec<String> {
    vec![
        trimmed.to_string(),
        signature_base.to_string(),
        tail.to_string(),
        remove_signature(tail),
    ]
}

impl InputPlan {
    fn push(&mut self, value: String, kind: &'static str) {
        if value.is_empty()
            || self
                .variants
                .iter()
                .any(|variant| variant.value == value && variant.kind == kind)
        {
            return;
        }
        if self.variants.len() >= MAX_COMPATIBLE_VARIANTS {
            self.truncated = true;
            return;
        }
        self.variants.push(InputVariant { value, kind });
    }
}
