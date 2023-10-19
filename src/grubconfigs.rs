use std::fmt::Write;
use std::os::unix::prelude::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use fn_error_context::context;
use openat_ext::OpenatDirExt;

/// The subdirectory of /boot we use
const GRUB2DIR: &str = "grub2";
const CONFIGDIR: &str = "/usr/lib/bootupd/grub2-static";
const DROPINDIR: &str = "configs.d";

#[context("Locating EFI vendordir")]
pub(crate) fn find_efi_vendordir(efidir: &openat::Dir) -> Result<PathBuf> {
    for d in efidir.list_dir(".")? {
        let d = d?;
        if d.file_name().as_bytes() == b"BOOT" {
            continue;
        }
        let meta = efidir.metadata(d.file_name())?;
        if !meta.is_dir() {
            continue;
        }
        return Ok(d.file_name().into());
    }
    anyhow::bail!("Failed to find EFI vendor dir")
}

/// Install the static GRUB config files.
#[context("Installing static GRUB configs")]
pub(crate) fn install(target_root: &openat::Dir, efi: bool) -> Result<()> {
    let bootdir = &target_root.sub_dir("boot").context("Opening /boot")?;

    let mut config = std::fs::read_to_string(Path::new(CONFIGDIR).join("grub-static-pre.cfg"))?;

    let dropindir = openat::Dir::open(&Path::new(CONFIGDIR).join(DROPINDIR))?;
    // Sort the files for reproducibility
    let mut entries = dropindir
        .list_dir(".")?
        .map(|e| e.map_err(anyhow::Error::msg))
        .collect::<Result<Vec<_>>>()?;
    entries.sort_by(|a, b| a.file_name().cmp(b.file_name()));
    for ent in entries {
        let name = ent.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| anyhow!("Invalid UTF-8: {name:?}"))?;
        if !name.ends_with(".cfg") {
            log::debug!("Ignoring {name}");
            continue;
        }
        writeln!(config, "source $prefix/{name}")?;
        dropindir
            .copy_file_at(name, bootdir, format!("{GRUB2DIR}/{name}"))
            .with_context(|| format!("Copying {name}"))?;
        println!("Installed {name}");
    }

    {
        let post = std::fs::read_to_string(Path::new(CONFIGDIR).join("grub-static-post.cfg"))?;
        config.push_str(post.as_str());
    }

    bootdir
        .write_file_contents(format!("{GRUB2DIR}/grub.cfg"), 0o644, config.as_bytes())
        .context("Copying grub-static.cfg")?;
    println!("Installed: grub.cfg");

    let efidir = efi
        .then(|| {
            target_root
                .sub_dir_optional("boot/efi/EFI")
                .context("Opening /boot/efi/EFI")
        })
        .transpose()?
        .flatten();
    if let Some(efidir) = efidir.as_ref() {
        let vendordir = find_efi_vendordir(efidir)?;
        log::debug!("vendordir={:?}", &vendordir);
        let target = &vendordir.join("grub.cfg");
        efidir
            .copy_file(&Path::new(CONFIGDIR).join("grub-static-efi.cfg"), target)
            .context("Copying static EFI")?;
        println!("Installed: {target:?}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn test_install() -> Result<()> {
        env_logger::init();
        let td = tempfile::tempdir()?;
        let tdp = td.path();
        let td = openat::Dir::open(tdp)?;
        std::fs::create_dir_all(tdp.join("boot/grub2"))?;
        std::fs::create_dir_all(tdp.join("boot/efi/EFI/BOOT"))?;
        std::fs::create_dir_all(tdp.join("boot/efi/EFI/fedora"))?;
        install(&td, true).unwrap();

        assert!(td.exists("boot/grub2/grub.cfg")?);
        assert!(td.exists("boot/efi/EFI/fedora/grub.cfg")?);
        Ok(())
    }
}
