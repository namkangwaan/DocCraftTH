# Changelog

All notable changes to **DocCraftTH (PdfCraft)** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **CJK System Font Fallback**: Added automatic system font fallback for East Asian glyphs (`SYSTEM_CJK_FALLBACK`) using Windows CJK font collections (`msyh.ttc`, `meiryo.ttc`, `msgothic.ttc`, `PingFang.ttc`, `wqy-microhei.ttc`). Fixes tofu boxes (□) for language options like `日本語`, `简体中文`, and `繁體中文` when running on non-CJK interface languages.
- **Thai Font Auto-Discovery**: Packaging script (`packaging/windows/package.ps1`) now auto-detects `craft-fonts` from sibling directories or the `CRAFT_FONTS_DIR` user environment variable.
- **Font Attributions in Packages**: Portable ZIP distributions automatically package OFL licenses for all bundled fonts (`Noto Sans Thai`, etc.).

### Changed
- **Directory Structure Reorganization**: Relocated project into consolidated structure under `K:\src\DocCraftTH\` (`pdfcraft` and `craft-fonts` siblings).
- Updated developer environment scripts (`scripts/dev-env/windows/setup-dev-env.ps1` and `remove-dev-env.ps1`) to reflect the new paths.

## [0.9.0] - 2026-10-21

### Added
- Thai language support and Thai PUA character mapping.
- Thai OCR synthetic testing and verification pipeline.
- WiX v5 installer and standalone portable Windows packaging automation.
