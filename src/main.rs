use acadrust::io::import::{self, ImportConfig, ImportFormat};
use acadrust::io::read::ReadOutcome;
use acadrust::{CadDocument, DwgReader, DwgWriter, DxfReader, DxfVersion, DxfWriter};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::{env, process};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const HELP: &str = "\
Usage:
  blackfox-dwg-test <input.dxf> <output.dwg> <version>
  blackfox-dwg-test convert [--binary-dxf] <input> <output.dxf|output.dwg> [version]
  blackfox-dwg-test inspect <input>
Versions: R11, R12, R13, R14, R2000, R2004, R2007, R2010, R2013, R2018,
          or the corresponding AC code. Omitting the version preserves it.";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Format {
    Dxf,
    Dwg,
    Exchange(ImportFormat),
}

fn main() {
    if let Err(error) = run(&env::args_os().skip(1).collect::<Vec<_>>()) {
        eprintln!("BlackFox conversion failed: {error}");
        process::exit(1);
    }
}

fn run(args: &[OsString]) -> Result<()> {
    match args {
        [flag] if flag == "--help" || flag == "-h" => {
            println!("{HELP}");
            Ok(())
        }
        [flag] if flag == "--version" => {
            println!("blackfox-dwg-test {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [cmd, input] if cmd == "inspect" => {
            let (format, doc) = read(Path::new(input))?;
            println!("{format:?} {}", doc.version.as_str());
            for (name, count) in entity_counts(&doc) {
                println!("{name}\t{count}");
            }
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "convert" => {
            let (binary_dxf, operands) = match rest {
                [flag, operands @ ..] if flag == "--binary-dxf" => (true, operands),
                _ => (false, rest),
            };
            match operands {
                [input, output] => {
                    convert(Path::new(input), Path::new(output), None, false, binary_dxf)
                }
                [input, output, target] => convert(
                    Path::new(input),
                    Path::new(output),
                    Some(version(target)?),
                    false,
                    binary_dxf,
                ),
                _ => Err(HELP.into()),
            }
        }
        [input, output, target] => convert(
            Path::new(input),
            Path::new(output),
            Some(version(target)?),
            true,
            false,
        ),
        _ => Err(HELP.into()),
    }
}

fn version(value: &OsString) -> Result<DxfVersion> {
    use DxfVersion::*;
    match value
        .to_str()
        .unwrap_or_default()
        .to_ascii_uppercase()
        .as_str()
    {
        "R11" | "R12" | "AC1009" => Ok(AC1009),
        "R13" | "AC1012" => Ok(AC1012),
        "R14" | "AC1014" => Ok(AC1014),
        "R2000" | "AC1015" => Ok(AC1015),
        "R2004" | "AC1018" => Ok(AC1018),
        "R2007" | "AC1021" => Ok(AC1021),
        "R2010" | "AC1024" => Ok(AC1024),
        "R2013" | "AC1027" => Ok(AC1027),
        "R2018" | "AC1032" => Ok(AC1032),
        _ => Err(format!("Unsupported CAD version: {}", value.to_string_lossy()).into()),
    }
}

fn detect(path: &Path) -> Result<Format> {
    let mut buffer = [0; 6];
    let mut file = File::open(path)?;
    let n = file.metadata()?.len().min(buffer.len() as u64) as usize;
    if n == 0 {
        return Err("Empty CAD input".into());
    }
    file.read_exact(&mut buffer[..n])?;
    let header = &buffer[..n];
    // DWG signatures are compact and precede the binary body. Both binary
    // DXF and textual DXF are parsed and validated by the DXF reader.
    if header.len() >= 6 && header.starts_with(b"AC") && header[2..6].iter().all(u8::is_ascii_digit)
    {
        Ok(Format::Dwg)
    } else if let Ok(format) = import::detect_format(path) {
        Ok(Format::Exchange(format))
    } else {
        Ok(Format::Dxf)
    }
}

fn output_format(path: &Path) -> Result<Format> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("dxf") => Ok(Format::Dxf),
        Some(ext) if ext.eq_ignore_ascii_case("dwg") => Ok(Format::Dwg),
        _ => Err("Destination must have a .dxf or .dwg extension".into()),
    }
}

fn read(path: &Path) -> Result<(Format, CadDocument)> {
    let format = detect(path)?;
    let ReadOutcome { document, stats } = match format {
        Format::Dxf => DxfReader::from_file(path)?.read_with_stats()?,
        Format::Dwg => DwgReader::from_file(path)?.read_with_stats()?,
        Format::Exchange(kind) => {
            let doc = import::import_file(path, &ImportConfig::default())?;
            for entry in doc.notifications.iter() {
                eprintln!("{kind:?}: {}", entry.message);
            }
            eprintln!("Warning: {kind:?} is imported as mesh geometry. Scene hierarchy, materials and metadata are limited to the upstream mesh importer.");
            if doc.entities().next().is_none() {
                return Err(format!("{kind:?} input contained no placed mesh geometry").into());
            }
            return Ok((format, doc));
        }
    };
    for diagnostic in &stats.diagnostics {
        eprintln!("{format:?} [{}]: {}", diagnostic.code, diagnostic.message);
    }
    if stats.skipped_source_records > 0 {
        eprintln!(
            "Warning: the reader skipped {} source records.",
            stats.skipped_source_records
        );
    }
    if document.version == DxfVersion::Unknown || stats.source_sections == 0 {
        return Err("Unrecognized CAD format or unsupported input version".into());
    }
    Ok((format, document))
}

fn entity_counts(doc: &CadDocument) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for entity in doc.entities() {
        *counts.entry(entity.as_entity().entity_type()).or_default() += 1;
    }
    counts
}

fn convert(
    input: &Path,
    output: &Path,
    target: Option<DxfVersion>,
    legacy: bool,
    binary_dxf: bool,
) -> Result<()> {
    let to = output_format(output)?;
    if binary_dxf && to != Format::Dxf {
        return Err("--binary-dxf requires a DXF destination".into());
    }
    if fs::canonicalize(output).ok().as_ref() == Some(&fs::canonicalize(input)?) {
        return Err("Input and destination refer to the same file".into());
    }
    let (from, mut doc) = read(input)?;
    if legacy && (from != Format::Dxf || to != Format::Dwg) {
        return Err("Use convert for operations other than DXF to DWG".into());
    }
    let counts = entity_counts(&doc);
    let source = doc.version;
    let target = target.unwrap_or(source);
    if target < source {
        eprintln!(
            "Warning: down-save {} to {} can approximate or omit unsupported records.",
            source.as_str(),
            target.as_str()
        );
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    doc.version = target;
    match to {
        Format::Dxf => {
            let writer = if binary_dxf {
                DxfWriter::new_binary(&doc)
            } else {
                DxfWriter::new(&doc)
            }
            .with_version(target);
            for omission in writer.omitted_architecture_records() {
                eprintln!("DXF omission: {omission:?}");
            }
            writer.write_to_writer(staged.as_file_mut())?;
        }
        Format::Dwg => DwgWriter::write_to_writer(staged.as_file_mut(), &doc)?,
        Format::Exchange(_) => unreachable!("destination format already validated"),
    }
    staged.as_file().sync_all()?;
    let (detected, reopened) = read(staged.path())?;
    if detected != to || reopened.version != target {
        return Err(format!(
            "Writer produced {detected:?} {}, expected {to:?} {}",
            reopened.version.as_str(),
            target.as_str()
        )
        .into());
    }
    let units_before = doc.header.insertion_units;
    let units_after = reopened.header.insertion_units;
    if units_before != units_after {
        eprintln!(
            "Warning: insertion units changed from {units_before} to {units_after} in {} output.",
            target.as_str()
        );
    }
    for name in doc
        .layers
        .names()
        .filter(|name| reopened.layers.get(name).is_none())
    {
        eprintln!("Warning: output has no layer named {name:?}.");
    }
    for name in doc
        .block_records
        .names()
        .filter(|name| reopened.block_records.get(name).is_none())
    {
        eprintln!("Warning: output has no block named {name:?}.");
    }
    let actual = entity_counts(&reopened);
    for name in counts
        .keys()
        .chain(actual.keys())
        .copied()
        .collect::<BTreeSet<_>>()
    {
        let (before, after) = (
            counts.get(name).copied().unwrap_or(0),
            actual.get(name).copied().unwrap_or(0),
        );
        if before != after {
            eprintln!("Warning: entity inventory changed for {name}: {before} -> {after}.");
        }
    }
    staged.persist(output)?;
    println!(
        "{from:?} {} -> {to:?} {}: {}",
        source.as_str(),
        target.as_str(),
        output.display()
    );
    Ok(())
}
