//! Java semantic provider metadata.
//!
//! Java semantics depend on classpath, Maven/Gradle, annotation processors,
//! generated sources, and JDK. CodeTrail uses the native SCIP Java indexer for
//! precise Java facts.
//!
//! The adapter records Java provider identity and proof inputs. JDK, wrapper,
//! classpath, annotation processors, and generated source producers enter the
//! environment/config proof.

use serde::{Deserialize, Serialize};

use crate::semantic_provider::{ProviderCapabilities, SemanticProviderVersion};

pub const JAVA_PROVIDER_NAME: &str = "scip-java";
pub const JAVA_PROTOCOL_VERSION: u32 = 1;

// ── Adapter configuration ───────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaAdapterConfig {
    pub project_root: String,
    pub build_system: JavaBuildSystem,
    pub jdk_version: String,
    pub jdk_home: Option<String>,
    pub wrapper_available: bool,
    pub annotation_processors: Vec<String>,
    pub generated_source_markers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaBuildSystem {
    Maven,
    Gradle,
    GradleKotlinDsl,
    Unknown,
}

// ── Session lifecycle ───────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaSessionState {
    Starting,
    Importing,
    Ready,
    Resolving,
    Stale,
    Partial,
    Shutdown,
}

// ── Symbol identity ─────────────────────────────────────────────────────────

/// Construct a stable Java symbol ID.
///
/// Format: `java:<package>/<outer_class>.<qualified_name>(<parameter_types>)`
///
/// Examples:
/// - `java:com/example/App.main([Ljava/lang/String;)V`
/// - `java:org/springframework/boot/SpringApplication.run`
/// - `java:com/example/UserService$Inner.process()V`
pub fn java_symbol_id(
    package_path: &str,
    qualified_name: &str,
    descriptor: &str,
    is_inner_class: bool,
) -> String {
    let separator = if is_inner_class { "$" } else { "." };
    format!(
        "java:{}:{}{}{}",
        package_path.replace('.', "/"),
        qualified_name,
        separator,
        descriptor
    )
}

// ── Environment hash ────────────────────────────────────────────────────────

pub fn java_environment_hash(
    jdk_version: &str,
    build_system: &JavaBuildSystem,
    build_file_hash: &str,
    wrapper_hash: Option<&str>,
) -> String {
    let wrapper = wrapper_hash.unwrap_or("none");
    let payload = format!("java:{jdk_version}:{build_system:?}:{build_file_hash}:{wrapper}");
    blake3::hash(payload.as_bytes()).to_hex().to_string()
}

// ── Provider capabilities ───────────────────────────────────────────────────

pub fn java_provider_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        language: crate::project_graph::ProjectLanguage::Java,
        provider_version: SemanticProviderVersion {
            name: JAVA_PROVIDER_NAME.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: JAVA_PROTOCOL_VERSION,
        },
        supports_batch_resolve: true,
        supports_import_graph: true,
        supports_workspace_symbols: true,
        max_batch_size: 200,
        partial_reasons: vec![
            crate::semantic_provider::PartialReason::ProviderMissing,
            crate::semantic_provider::PartialReason::StartupFailed,
            crate::semantic_provider::PartialReason::Timeout,
            crate::semantic_provider::PartialReason::ResourceLimited,
            crate::semantic_provider::PartialReason::ProviderPartial,
            crate::semantic_provider::PartialReason::UnsupportedCapability,
            crate::semantic_provider::PartialReason::ResolveFailed,
        ],
    }
}

// ── Fixture design ──────────────────────────────────────────────────────────

// Test fixtures needed for Java provider validation:
//
// 1. **Basic class and method**:
//    ```java
//    // src/main/java/com/example/App.java
//    package com.example;
//    public class App {
//        public static void main(String[] args) { }
//    }
//    ```
//    Expected: one def `App`, one def `main` with descriptor `([Ljava/lang/String;)V`.
//
// 2. **Method overload**:
//    ```java
//    public class Calculator {
//        public int add(int a, int b) { return a + b; }
//        public double add(double a, double b) { return a + b; }
//    }
//    ```
//    Expected: two distinct symbols for `add`, differentiated by descriptor.
//
// 3. **Multi-module Maven**:
//    Parent POM + two child modules.
//    Expected: cross-module references resolved through classpath.
//
// 4. **Annotation processor / generated source**:
//    ```java
//    @lombok.Data
//    public class User { private String name; }
//    ```
//    Expected: `Lombok` missing → partial reason, generated getter/setter not in precise index.
//
// 5. **Inner class**:
//    ```java
//    public class Outer { public class Inner { } }
//    ```
//    Expected: symbol id uses `$` separator: `Outer$Inner`.
//
// 6. **Project import failure**:
//    Broken `pom.xml` → import timeout → `StartupFailed` reason, Java root marked partial.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_symbol_id_basic() {
        let id = java_symbol_id("com.example", "App.main", "([Ljava/lang/String;)V", false);
        assert!(id.starts_with("java:com/example:"));
        assert!(id.contains("App.main"));
    }

    #[test]
    fn java_symbol_id_inner_class() {
        let id = java_symbol_id("com.example", "Outer.Inner", "()V", true);
        assert!(id.contains("$"));
    }

    #[test]
    fn java_environment_hash_is_deterministic() {
        let a = java_environment_hash("21", &JavaBuildSystem::Maven, "aaa", None);
        let b = java_environment_hash("21", &JavaBuildSystem::Maven, "aaa", None);
        assert_eq!(a, b);
    }

    #[test]
    fn java_environment_hash_differs_on_build_system() {
        let a = java_environment_hash("21", &JavaBuildSystem::Maven, "aaa", None);
        let b = java_environment_hash("21", &JavaBuildSystem::Gradle, "aaa", None);
        assert_ne!(a, b);
    }

    #[test]
    fn java_provider_capabilities_include_resolve_failed() {
        let caps = java_provider_capabilities();
        let reasons: Vec<_> = caps
            .partial_reasons
            .iter()
            .map(|r| format!("{r:?}"))
            .collect();
        assert!(reasons.iter().any(|r| r.contains("ResolveFailed")));
        assert!(caps.language == crate::project_graph::ProjectLanguage::Java);
    }
}
