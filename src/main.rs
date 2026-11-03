use acadrust::{DwgWriter, DxfReader, DxfVersion};
use std::{env, process};

fn main() {
    if let Err(error) = run() {
        eprintln!("BlackFox conversion failed: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() != 4 {
        eprintln!("Usage: blackfox-dwg-test <input.dxf> <output.dwg> <version>");
        eprintln!("Versions: R12 R2000 R2004 R2007 R2010 R2013 R2018");
        process::exit(2);
    }

    let version = match args[3].to_uppercase().as_str() {
        "R12"   => DxfVersion::AC1009,
        "R2000" => DxfVersion::AC1015,
        "R2004" => DxfVersion::AC1018,
        "R2007" => DxfVersion::AC1021,
        "R2010" => DxfVersion::AC1024,
        "R2013" => DxfVersion::AC1027,
        "R2018" => DxfVersion::AC1032,
        _ => return Err(format!("Unsupported DWG version: {}", args[3]).into()),
    };

    let mut document = DxfReader::from_file(&args[1])?.read()?;

    println!("Input DXF version: {:?}", document.version);
    println!("Target DWG version: {:?}", version);

    document.version = version;

    DwgWriter::write_to_file(&args[2], &document)?;

    println!("Conversion completed: {}", args[2]);
    Ok(())
}
