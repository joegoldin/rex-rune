//! Image paste follows Pi's file-path contract, with explicit transport to remote hosts.
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

pub const IMAGE_LIMIT: usize = 32 * 1024 * 1024;
const REMOTE_SCRIPT: &str = r#"umask 077
d=$(mktemp -d "${TMPDIR:-/tmp}/rex-paste.XXXXXXXX") || exit 1
cd "$d" || exit 1
d=$(pwd -P) || exit 1
p="$d/clipboard.png"
trap 'rm -f "$p"; rmdir "$d"' EXIT HUP INT TERM
cat > "$p" || exit 1
printf '%s\n' "$p" || exit 1
trap - EXIT HUP INT TERM"#;

pub fn validate_png(bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() <= IMAGE_LIMIT, "Clipboard image exceeds 32 MiB");
    ensure!(
        bytes.len() >= 24 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" && &bytes[12..16] == b"IHDR",
        "Clipboard image is not PNG"
    );
    let width = u32::from_be_bytes(bytes[16..20].try_into()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into()?);
    ensure!(
        width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 64 * 1024 * 1024,
        "Clipboard image exceeds 64 megapixels"
    );
    Ok(())
}

pub fn validate_ssh_target(target: &str) -> Result<()> {
    ensure!(
        !target.is_empty()
            && target.len() <= 512
            && !target.starts_with('-')
            && target
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-@:[%]+".contains(c)),
        "Use an SSH config alias or user@host; configure ports and keys in your SSH config"
    );
    Ok(())
}

/// A path is pasted as a shell literal without executing it or appending Enter.
pub fn quote_path(path: &str) -> Result<String> {
    ensure!(
        Path::new(path).is_absolute() && !path.chars().any(char::is_control),
        "Image path is invalid"
    );
    if path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_-/.:".contains(c))
    {
        return Ok(path.to_owned());
    }
    Ok(quote_argument(path))
}
fn quote_argument(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// A fresh 0700 directory and 0600 file; never reuse or overwrite clipboard files.
pub fn write_local(bytes: &[u8], base: &Path) -> Result<PathBuf> {
    validate_png(bytes)?;
    let base = fs::canonicalize(base).context("Image temporary directory is unavailable")?;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    for _ in 0..16 {
        let directory = base.join(format!(
            "rex-paste-{}-{time}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&directory) {
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            result => result.context("Could not create a private image directory")?,
        }
        let path = directory.join("clipboard.png");
        let result = (|| {
            quote_path(path.to_str().context("Image temporary path is not UTF-8")?)?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&path)?;
            file.write_all(bytes)?;
            file.flush()?;
            Ok(path.clone())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_dir(&directory);
        }
        return result;
    }
    bail!("Could not allocate a private image directory")
}

/// OpenSSH handles host keys and authentication. No password prompts, forwards or shell input.
pub async fn upload_ssh(bytes: &[u8], target: &str) -> Result<PathBuf> {
    upload_with("ssh", bytes, target, Duration::from_secs(20)).await
}

async fn upload_with(
    executable: &str,
    bytes: &[u8],
    target: &str,
    timeout: Duration,
) -> Result<PathBuf> {
    validate_png(bytes)?;
    validate_ssh_target(target)?;
    let mut child = Command::new(executable)
        .args([
            "-T",
            "-oBatchMode=yes",
            "-oStrictHostKeyChecking=yes",
            "-oConnectTimeout=10",
            "-oClearAllForwardings=yes",
            "-oPermitLocalCommand=no",
            "-oRemoteCommand=none",
            "-oStdinNull=no",
            "-oSessionType=default",
            "--",
            target,
        ])
        .arg(format!("exec sh -c {}", quote_argument(REMOTE_SCRIPT)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Could not start OpenSSH for image upload")?;
    let transfer = async {
        let mut stdin = child
            .stdin
            .take()
            .context("SSH image input is unavailable")?;
        stdin
            .write_all(bytes)
            .await
            .context("SSH image upload failed")?;
        stdin.shutdown().await?;
        drop(stdin);
        let mut response = Vec::new();
        child
            .stdout
            .take()
            .context("SSH image response is unavailable")?
            .take(4097)
            .read_to_end(&mut response)
            .await?;
        ensure!(response.len() <= 4096, "SSH image response is too large");
        ensure!(
            child.wait().await?.success(),
            "SSH image upload failed; check the target, known host key and key-based access"
        );
        let path = std::str::from_utf8(&response)?
            .strip_suffix('\n')
            .context("SSH image response is invalid")?;
        ensure!(
            path.ends_with("/clipboard.png"),
            "SSH image response is invalid"
        );
        quote_path(path)?;
        Ok(PathBuf::from(path))
    };
    let result = tokio::time::timeout(timeout, transfer)
        .await
        .context("SSH image upload timed out")
        .and_then(|r| r);
    if result.is_err() {
        let _ = child.kill().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png() -> Vec<u8> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC").unwrap()
    }
    #[test]
    fn local_images_are_private_distinct_complete_and_quoted() {
        let a = write_local(&png(), &std::env::temp_dir()).unwrap();
        let b = write_local(&png(), &std::env::temp_dir()).unwrap();
        assert_ne!(a, b);
        assert_eq!(fs::read(&a).unwrap(), png());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&a).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(a.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert_eq!(
            quote_path("/tmp/a b'$(touch nope).png").unwrap(),
            "'/tmp/a b'\\''$(touch nope).png'"
        );
        for path in ["relative.png", "/tmp/a\ncommand", "/tmp/a\x1bcommand"] {
            assert!(quote_path(path).is_err());
        }
        for path in [a, b] {
            fs::remove_file(&path).unwrap();
            fs::remove_dir(path.parent().unwrap()).unwrap();
        }
    }
    #[test]
    fn invalid_images_and_ssh_options_are_rejected() {
        for target in [
            "",
            "-oProxyCommand=bad",
            "host command",
            "host\ncommand",
            "user@host;command",
            "$(command)",
        ] {
            assert!(validate_ssh_target(target).is_err());
        }
        for target in ["elphael", "joe@erdtree", "user@[::1]"] {
            validate_ssh_target(target).unwrap();
        }
        assert!(validate_png(b"not an image").is_err());
        let mut image = png();
        image[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(validate_png(&image).is_err());
        let mut image = png();
        image.resize(IMAGE_LIMIT + 1, 0);
        assert!(validate_png(&image).is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn ssh_upload_is_complete_bounded_and_reaps_timed_out_children() {
        use std::os::unix::fs::PermissionsExt;
        let seed = write_local(&png(), &std::env::temp_dir()).unwrap();
        let dir = seed.parent().unwrap();
        let executable = dir.join("ssh-fixture");
        let delivered = dir.join("delivered.png");
        let pid_file = dir.join("child.pid");
        let shell = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|p| p.join("sh"))
            .find(|p| p.is_file())
            .expect("POSIX shell on PATH");
        let script = |body: &str| {
            fs::write(&executable, format!("#!{}\n{body}\n", shell.display())).unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        };
        script(&format!(
            "cat > {}\nprintf '%s\\n' '/remote image/clipboard.png'",
            quote_argument(delivered.to_str().unwrap())
        ));
        let path = upload_with(
            executable.to_str().unwrap(),
            &png(),
            "user@fixture",
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(path, Path::new("/remote image/clipboard.png"));
        assert_eq!(fs::read(&delivered).unwrap(), png());
        for body in [
            "cat >/dev/null; printf '/remote/clipboard.png\\n'; exit 7",
            "cat >/dev/null; printf '/remote/clipboard.png\\nINJECTED\\n'",
            "cat >/dev/null; head -c 5000 /dev/zero",
        ] {
            script(body);
            assert!(
                upload_with(
                    executable.to_str().unwrap(),
                    &png(),
                    "fixture",
                    Duration::from_secs(2)
                )
                .await
                .is_err()
            );
        }
        script(&format!(
            "echo $$ > {}; exec sleep 5",
            quote_argument(pid_file.to_str().unwrap())
        ));
        let start = std::time::Instant::now();
        assert!(
            upload_with(
                executable.to_str().unwrap(),
                &png(),
                "fixture",
                Duration::from_millis(80)
            )
            .await
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        #[cfg(target_os = "linux")]
        {
            let pid = fs::read_to_string(&pid_file).unwrap();
            assert!(!Path::new("/proc").join(pid.trim()).exists());
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
