# BlackFox DWG Converter

A lightweight, open-source DXF-to-DWG converter built in Rust using **acadrust**.

## Overview

BlackFox DWG Converter provides a standalone conversion component for the **BlackFox CAD Exporter** extension for IngeTrazo.

The converter is distributed as platform-specific binaries and is downloaded by the extension only after the user grants permission.

## Features

- DXF-to-DWG conversion
- Standalone command-line converter
- Cross-platform binary distribution
- SHA-256 checksum verification by the BlackFox CAD Exporter
- No installation required when used through the extension

## Downloads

Precompiled binaries are available for:

- Windows — x86-64 and ARM64
- Linux — x86-64 and ARM64
- macOS — x86-64 and ARM64

Download the appropriate archive from the [Releases](https://github.com/davidsmavrov-debug/blackfox-dwg-converter/releases) page.

## Integration

The converter is used by [BlackFox CAD Exporter](https://github.com/ingelibre/ingetrazo), an extension for the IngeTrazo CAD application.

When DWG export is requested, the extension asks permission before downloading the converter, verifies the downloaded archive and caches the converter for subsequent use.

## Licensing

BlackFox DWG Converter uses third-party open-source components subject to their respective licences.

See the repository's licence and third-party notices for applicable terms.

## Project status

Version 1.0.0 is the published converter release.

This repository contains the standalone DWG conversion component used by BlackFox CAD Exporter.
