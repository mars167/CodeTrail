pub(super) fn is_workflow(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with(".github/workflows/")
        || lower.ends_with("/.gitlab-ci.yml")
        || lower.ends_with("/.gitlab-ci.yaml")
        || lower == ".gitlab-ci.yml"
        || lower == ".gitlab-ci.yaml"
}

pub(super) fn is_compose(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with("docker-compose.yml")
        || lower.ends_with("docker-compose.yaml")
        || lower.ends_with("compose.yml")
        || lower.ends_with("compose.yaml")
}

pub(super) fn is_kubernetes_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("k8s/")
        || lower.starts_with("kubernetes/")
        || lower.contains("/k8s/")
        || lower.contains("/kubernetes/")
}

pub(super) fn looks_like_kubernetes(source: &str) -> bool {
    source.contains("apiVersion:") && source.contains("kind:") && source.contains("metadata:")
}

pub(super) fn is_dockerfile(path: &str) -> bool {
    let name = file_name(path);
    name == "Dockerfile" || name.starts_with("Dockerfile.")
}

pub(super) fn is_makefile(path: &str) -> bool {
    let name = file_name(path);
    name == "Makefile" || name == "makefile" || name.ends_with(".mk")
}

pub(super) fn is_shell_script(path: &str, source: &str) -> bool {
    matches!(extension(path).as_deref(), Some("sh" | "bash" | "zsh"))
        || source.starts_with("#!/bin/sh")
        || source.starts_with("#!/usr/bin/env sh")
        || source.starts_with("#!/usr/bin/env bash")
        || source.starts_with("#!/bin/bash")
}

pub(super) fn is_ini_like(path: &str) -> bool {
    matches!(
        extension(path).as_deref(),
        Some("ini" | "properties" | "conf" | "config" | "env")
    ) || file_name(path).starts_with(".env")
}

pub(super) fn extension(path: &str) -> Option<String> {
    file_name(path)
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
}

pub(super) fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
