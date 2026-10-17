use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};

const PATH_FILE: &str = "CLIPath.txt";
const CLI_ZIP_URL: &str = "https://github.com/arduino/arduino-cli/releases/download/v0.35.3/arduino-cli_0.35.3_Windows_64bit.zip";
const SKETCH_ZIP_URL: &str = "https://github.com/huh445/Digispark-Scripts/archive/refs/heads/main.zip";
const BOARD: &str = "digistump:avr:digispark-tiny";
const BOARD_INDEX_URL: &str = "https://raw.githubusercontent.com/digistump/arduino-boards-index/master/package_digistump_index.json";

/// Directory the tool keeps its state in (next to the executable, like the old script).
fn base_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn cli_exe() -> PathBuf {
    let name = if cfg!(windows) { "arduino-cli.exe" } else { "arduino-cli" };
    base_dir().join("arduino-cli").join(name)
}

fn run_cmd(program: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {}", program.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    println!("{stdout}");
    if !out.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&out.stderr));
        bail!("command failed with {}", out.status);
    }
    Ok(stdout)
}

fn download_file(url: &str, dest: &Path) -> Result<()> {
    let resp = ureq::get(url).call().with_context(|| format!("GET {url}"))?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok());
    let name = dest.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let pb = match total {
        Some(n) => ProgressBar::new(n),
        None => ProgressBar::new_spinner(),
    };
    pb.set_style(
        ProgressStyle::with_template("{msg} {bytes}/{total_bytes} [{bar:30}] {eta}")?
            .progress_chars("=> "),
    );
    pb.set_message(name);

    let mut reader = resp.into_reader();
    let mut out = File::create(dest)?;
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        pb.inc(n as u64);
    }
    pb.finish();
    println!("Downloaded to {}", dest.display());
    Ok(())
}

fn unzip(src: &Path, dest: &Path) -> Result<()> {
    if fs::metadata(src).map(|m| m.len()).unwrap_or(0) == 0 {
        bail!("{} is empty or missing", src.display());
    }
    let mut archive = zip::ZipArchive::new(File::open(src)?)?;
    let pb = ProgressBar::new(archive.len() as u64);
    pb.set_style(ProgressStyle::with_template("Extracting {pos}/{len} files [{bar:30}]")?.progress_chars("=> "));
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        // enclosed_name rejects entries that would escape `dest` (zip-slip).
        let Some(rel) = file.enclosed_name() else { continue };
        let target = dest.join(rel);
        if file.is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            io::copy(&mut file, &mut File::create(&target)?)?;
        }
        pb.inc(1);
    }
    pb.finish();
    println!("Extraction complete");
    Ok(())
}

fn install_cli() -> Result<PathBuf> {
    if !cfg!(windows) {
        bail!("automatic arduino-cli install is Windows-only; install arduino-cli yourself and put it on PATH");
    }
    let base = base_dir();
    let cli_dir = base.join("arduino-cli");
    let zip_file = base.join("arduino-cli.zip");
    let exe = cli_exe();

    let _ = fs::remove_dir_all(&cli_dir);
    println!("Downloading and installing Arduino-CLI...");
    download_file(CLI_ZIP_URL, &zip_file)?;
    unzip(&zip_file, &cli_dir)?;
    fs::remove_file(&zip_file)?;

    let config = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|h| PathBuf::from(h).join(".arduino15").join("arduino-cli.yaml"));
    if config.as_ref().is_some_and(|c| c.exists()) {
        println!("Arduino CLI config already exists, skipping init.");
    } else {
        run_cmd(&exe, &["config", "init"])?;
    }
    run_cmd(&exe, &["config", "add", "board_manager.additional_urls", BOARD_INDEX_URL])?;
    run_cmd(&exe, &["core", "install", "digistump:avr"])?;
    fs::write(PATH_FILE, exe.to_string_lossy().as_bytes())?;
    println!("Arduino-CLI installed at {}", exe.display());
    Ok(exe)
}

fn has_digispark(cli: &Path) -> bool {
    run_cmd(cli, &["core", "list"])
        .map(|out| out.lines().any(|l| l.contains("digistump:avr")))
        .unwrap_or(false)
}

fn verify_cli() -> Result<PathBuf> {
    if let Ok(saved) = fs::read_to_string(PATH_FILE) {
        let cli = PathBuf::from(saved.trim().trim_matches('"'));
        if cli.exists() && has_digispark(&cli) {
            return Ok(cli);
        }
    }
    let local = cli_exe();
    for cli in [local.clone(), PathBuf::from("arduino-cli")] {
        if (cli.exists() || cli.components().count() == 1) && has_digispark(&cli) {
            fs::write(PATH_FILE, cli.to_string_lossy().as_bytes())?;
            return Ok(cli);
        }
    }
    install_cli()
}

fn fetch_sketches(dir: &Path) -> Result<()> {
    let _ = fs::remove_dir_all(dir);
    println!("Fetching sketches from GitHub...");
    let zip_file = base_dir().join("sketches.zip");
    download_file(SKETCH_ZIP_URL, &zip_file)?;
    unzip(&zip_file, dir)?;
    fs::remove_file(&zip_file)?;
    Ok(())
}

fn list_sketches(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            list_sketches(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "ino") {
            out.push(path);
        }
    }
    Ok(())
}

fn choose_sketch(sketches: &[PathBuf]) -> Result<&PathBuf> {
    for (i, s) in sketches.iter().enumerate() {
        println!("{}. {}", i + 1, s.file_name().unwrap_or_default().to_string_lossy());
    }
    print!("Select sketch number: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let n: usize = line.trim().parse().context("not a number")?;
    sketches
        .get(n.wrapping_sub(1))
        .context("selection out of range")
}

fn compile_and_upload(cli: &Path, sketch: &Path) -> Result<()> {
    let sketch = sketch.to_string_lossy();
    run_cmd(cli, &["compile", "-b", BOARD, &sketch])?;
    println!("Compilation successful.");
    println!("Please plug in Digispark now...");
    sleep(Duration::from_secs(2));
    run_cmd(cli, &["upload", "-b", BOARD, &sketch])?;
    println!("Upload complete.");
    Ok(())
}

fn main() -> Result<()> {
    let cli = verify_cli()?;
    let sketch_dir = base_dir().join("sketches");
    fetch_sketches(&sketch_dir)?;
    let mut sketches = Vec::new();
    list_sketches(&sketch_dir, &mut sketches)?;
    if sketches.is_empty() {
        bail!("No sketches found.");
    }
    sketches.sort();
    let sketch = choose_sketch(&sketches)?;
    compile_and_upload(&cli, sketch)
}
