use aurora_extension_runtime::{Manifest, validate};
use semver::Version;
use std::{
    collections::BTreeMap,
    env,
    error::Error,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};
use zip::{DateTime, ZipWriter, write::SimpleFileOptions};

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("aurora-ext: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<OsString>) -> Result<(), Box<dyn Error>> {
    let Some(command) = args.first().and_then(|value| value.to_str()) else {
        print_help();
        return Ok(());
    };
    match command {
        "pack" => pack(parse_options(args.into_iter().skip(1))?),
        "validate" => {
            let package = args.get(1).map(PathBuf::from).ok_or("validate requires a package path")?;
            let options = parse_options(args.into_iter().skip(2))?;
            reject_unknown_options(&options, &["--api-version"])?;
            let api = parse_api_version(options.get("--api-version"))?;
            validate_package(&package, &api)
        }
        "--help" | "-h" | "help" => {
            print_help();
            Ok(())
        }
        _ => Err(format!("unknown command `{command}`; use --help").into()),
    }
}

fn print_help() {
    println!(
        "Aurora extension package tool\n\
         Usage:\n\
         \x20 aurora-ext pack --manifest <manifest.json> --module <module.wasm> --output <package.ext> [--assets <directory>] [--api-version <semver>]\n\
         \x20 aurora-ext validate <package.ext> [--api-version <semver>]\n\
         `pack` creates a deterministic ZIP and validates it before writing. The publisher field is not authenticated."
    );
}

fn parse_options(mut args: impl Iterator<Item = OsString>) -> Result<BTreeMap<String, PathBuf>, Box<dyn Error>> {
    let mut options = BTreeMap::new();
    while let Some(key) = args.next() {
        let key = key.into_string().map_err(|_| "option name is not valid UTF-8")?;
        if !key.starts_with("--") {
            return Err(format!("unexpected argument `{key}`").into());
        }
        let value = args.next().ok_or_else(|| format!("missing value for `{key}`"))?;
        if options.insert(key.clone(), PathBuf::from(value)).is_some() {
            return Err(format!("duplicate option `{key}`").into());
        }
    }
    Ok(options)
}

fn parse_api_version(options: Option<&PathBuf>) -> Result<Version, Box<dyn Error>> {
    let value = options.map_or_else(|| "1.0.0".to_owned(), |path| path.to_string_lossy().into_owned());
    Ok(Version::parse(&value)?)
}

fn pack(options: BTreeMap<String, PathBuf>) -> Result<(), Box<dyn Error>> {
    let manifest_path = required(&options, "--manifest")?;
    let module_path = required(&options, "--module")?;
    let output_path = required(&options, "--output")?;
    let api = parse_api_version(options.get("--api-version"))?;
    reject_unknown_options(&options, &["--manifest", "--module", "--output", "--assets", "--api-version"])?;

    let manifest = fs::read(manifest_path)?;
    let _: Manifest = serde_json::from_slice(&manifest)?;
    let module = fs::read(module_path)?;
    let mut members = BTreeMap::new();
    members.insert("manifest.json".to_owned(), manifest);
    members.insert("module.wasm".to_owned(), module);
    if let Some(assets) = options.get("--assets") {
        collect_assets(assets, assets, &mut members)?;
    }
    let archive = build_archive(&members)?;
    let validated = validate(&archive, &api)?;
    write_new_atomic(output_path, &archive)?;
    println!(
        "packed {} ({}) {} -> {}",
        validated.manifest.name,
        validated.manifest.id,
        validated.manifest.version,
        output_path.display()
    );
    println!("content sha256: {}", hex(&validated.content_sha256));
    Ok(())
}

fn validate_package(package_path: &Path, api: &Version) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(package_path)?;
    let validated = validate(&bytes, api)?;
    println!(
        "valid {} ({}) {}",
        validated.manifest.name, validated.manifest.id, validated.manifest.version
    );
    println!("publisher claim: {} (unverified)", validated.manifest.publisher);
    println!("requested permissions: {}", validated.manifest.permissions.join(", "));
    println!("content sha256: {}", hex(&validated.content_sha256));
    Ok(())
}

fn required<'a>(options: &'a BTreeMap<String, PathBuf>, key: &str) -> Result<&'a Path, Box<dyn Error>> {
    options
        .get(key)
        .map(PathBuf::as_path)
        .ok_or_else(|| format!("pack requires {key}").into())
}

fn reject_unknown_options(options: &BTreeMap<String, PathBuf>, allowed: &[&str]) -> Result<(), Box<dyn Error>> {
    if let Some(key) = options.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("unknown option `{key}`").into());
    }
    Ok(())
}

fn collect_assets(root: &Path, directory: &Path, members: &mut BTreeMap<String, Vec<u8>>) -> Result<(), Box<dyn Error>> {
    let mut entries: Vec<_> = fs::read_dir(directory)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(format!("asset symlinks are not allowed: {}", path.display()).into());
        }
        if kind.is_dir() {
            collect_assets(root, &path, members)?;
        } else if kind.is_file() {
            let relative = path.strip_prefix(root)?;
            let relative = relative.to_str().ok_or("asset path is not valid UTF-8")?.replace('\\', "/");
            members.insert(format!("assets/{relative}"), fs::read(&path)?);
        } else {
            return Err(format!("unsupported asset type: {}", path.display()).into());
        }
    }
    Ok(())
}

fn build_archive(members: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in members {
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(DateTime::default())
            .unix_permissions(0o100644);
        writer.start_file(path.as_str(), options)?;
        writer.write_all(bytes)?;
    }
    Ok(writer.finish()?.into_inner())
}

fn write_new_atomic(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    if path.exists() {
        return Err(format!("refusing to overwrite existing output: {}", path.display()).into());
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path.file_name().ok_or("output path has no filename")?.to_string_lossy();
    let temporary = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| -> Result<(), Box<dyn Error>> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::ZipArchive;

    #[test]
    fn package_zip_is_reproducible_and_integrity_checked() {
        let mut members = BTreeMap::new();
        members.insert("manifest.json".to_owned(), br#"{"manifest_version":1}"#.to_vec());
        members.insert("module.wasm".to_owned(), b"not wasm".to_vec());
        let first = build_archive(&members).expect("first deterministic archive");
        let second = build_archive(&members).expect("second deterministic archive");
        assert_eq!(first, second);
        let mut archive = ZipArchive::new(Cursor::new(first)).expect("valid ZIP container");
        assert_eq!(archive.len(), 2);
        assert!(archive.by_name("manifest.json").is_ok());
    }

    #[test]
    fn pack_validation_rejects_an_invalid_guest_module() {
        let bytes = build_archive(&BTreeMap::from([
            (
                "manifest.json".to_owned(),
                br#"{"manifest_version":1,"id":"dev.example.fixture","name":"Extension Fixture","version":"0.1.0","api":"*","entry":"module.wasm","permissions":[],"network_domains":[],"publisher":"Fixture"}"#.to_vec(),
            ),
            ("module.wasm".to_owned(), b"not wasm".to_vec()),
        ]))
        .expect("archive");
        assert!(validate(&bytes, &Version::new(1, 0, 0)).is_err());
    }
}
