//! ingot — split out of src/main.rs (pure move).
//! Behaviour unchanged; imports added per cargo check.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(feature = "native")]
use taida::codegen;
use taida::pkg;

use crate::cli::help::{
    print_deps_help, print_ingot_help, print_install_help, print_publish_help, print_update_help,
};
use crate::is_help_flag;

pub(crate) fn run_ingot(args: &[String]) {
    if args.is_empty() || is_help_flag(args[0].as_str()) {
        print_ingot_help();
        return;
    }

    match args[0].as_str() {
        "deps" => run_deps(&args[1..]),
        "install" => run_install(&args[1..]),
        "migrate-lockfile" => run_migrate_lockfile(&args[1..]),
        "update" => run_update(&args[1..]),
        #[cfg(feature = "community")]
        "publish" => run_publish(&args[1..]),
        #[cfg(not(feature = "community"))]
        "publish" => {
            eprintln!("The 'taida ingot publish' command requires the 'community' feature.");
            eprintln!("Rebuild with: cargo build --features community");
            std::process::exit(1);
        }
        "cache" => run_cache(&args[1..]),
        other => {
            eprintln!("Unknown subcommand for `taida ingot`: {}", other);
            eprintln!("Run `taida ingot --help` for usage.");
            std::process::exit(2);
        }
    }
}

pub(crate) fn run_deps(args: &[String]) {
    match args {
        [] => {}
        [arg] if is_help_flag(arg.as_str()) => {
            print_deps_help();
            return;
        }
        _ => {
            eprintln!("Unexpected arguments.");
            eprintln!("Run `taida ingot deps --help` for usage.");
            std::process::exit(1);
        }
    }

    // Find project root by looking for packages.tdm
    let project_dir = find_packages_tdm().unwrap_or_else(|| {
        eprintln!("No packages.tdm found in current directory or parent directories.");
        eprintln!("Run 'taida init' to create a new project.");
        std::process::exit(1);
    });

    // Parse manifest
    let manifest = match pkg::manifest::Manifest::from_dir(&project_dir) {
        Ok(Some(m)) => m,
        Ok(None) => {
            eprintln!("No packages.tdm found in '{}'", project_dir.display());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error parsing packages.tdm: {}", e);
            std::process::exit(1);
        }
    };

    if manifest.deps.is_empty() {
        println!("No dependencies defined in packages.tdm");
        return;
    }

    println!("Resolving dependencies for '{}'...", manifest.name);

    // Resolve dependencies using provider chain
    let result = pkg::resolver::resolve_deps(&manifest);

    // Report errors
    for err in &result.errors {
        eprintln!("  ERROR: {}", err);
    }

    // Strict mode for `taida ingot deps`: never install or write lockfile on resolve errors.
    if !result.errors.is_empty() {
        eprintln!("Dependency resolution failed. Skipping install and lockfile update.");
        std::process::exit(1);
    }

    // Install resolved dependencies
    if !result.resolved.is_empty() {
        match pkg::resolver::install_deps(&manifest, &result) {
            Ok(()) => {
                for (name, path) in &result.resolved {
                    println!("  {} -> {}", name, path.display());
                }
                println!(
                    "Installed {} dependency(ies) in .taida/deps/",
                    result.resolved.len()
                );
            }
            Err(e) => {
                eprintln!("Error installing dependencies: {}", e);
                std::process::exit(1);
            }
        }

        // Generate lockfile
        match pkg::resolver::write_lockfile(&manifest, &result) {
            Ok(()) => println!("Updated taida.lock"),
            Err(e) => eprintln!("Warning: could not write lockfile: {}", e),
        }
    }
}

pub(crate) fn run_install(args: &[String]) {
    // RC1.5-3c: parse --force-refresh flag
    // RC2.7-4a: parse --allow-local-addon-build flag
    // C17-2: parse --no-remote-check (mutually exclusive with --force-refresh)
    let mut force_refresh = false;
    let mut no_remote_check = false;
    let mut allow_local_addon_build = false;
    let mut allow_fresh = false;
    let mut frozen = false;
    let mut filtered: Vec<&str> = Vec::new();
    for arg in args {
        if arg == "--force-refresh" {
            force_refresh = true;
        } else if arg == "--no-remote-check" {
            no_remote_check = true;
        } else if arg == "--allow-local-addon-build" {
            allow_local_addon_build = true;
        } else if arg == "--allow-fresh" {
            allow_fresh = true;
        } else if arg == "--frozen" {
            frozen = true;
        } else if is_help_flag(arg.as_str()) {
            print_install_help();
            return;
        } else {
            filtered.push(arg.as_str());
        }
    }
    if !filtered.is_empty() {
        eprintln!("Unexpected arguments.");
        eprintln!("Run `taida ingot install --help` for usage.");
        std::process::exit(1);
    }
    // C17-2: mutual exclusion is a hard error so users cannot silently
    // combine the two refresh knobs with surprising semantics.
    let refresh_flags = pkg::resolver::StoreRefreshFlags {
        force_refresh,
        no_remote_check,
    };
    if let Err(msg) = refresh_flags.validate() {
        eprintln!("Error: {}", msg);
        eprintln!("Run `taida ingot install --help` for usage.");
        std::process::exit(1);
    }

    // Find project root by looking for packages.tdm
    let project_dir = find_packages_tdm().unwrap_or_else(|| {
        eprintln!("No packages.tdm found in current directory or parent directories.");
        eprintln!("Run 'taida init' to create a new project.");
        std::process::exit(1);
    });

    // Parse manifest
    let manifest = match pkg::manifest::Manifest::from_dir(&project_dir) {
        Ok(Some(m)) => m,
        Ok(None) => {
            eprintln!("No packages.tdm found in '{}'", project_dir.display());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error parsing packages.tdm: {}", e);
            std::process::exit(1);
        }
    };

    if manifest.deps.is_empty() {
        println!("No dependencies defined in packages.tdm");
        let lock_path = project_dir.join(".taida").join("taida.lock");
        if frozen {
            match pkg::lockfile::Lockfile::read(&lock_path) {
                Ok(Some(lockfile)) if lockfile.is_up_to_date(&[]) => {
                    println!("taida.lock is frozen and up to date");
                    return;
                }
                Ok(Some(_)) => {
                    eprintln!(
                        "[E32K2_LOCKFILE_DRIFT] --frozen requires .taida/taida.lock to match packages.tdm"
                    );
                    std::process::exit(1);
                }
                Ok(None) => {
                    eprintln!(
                        "[E32K2_LOCKFILE_DRIFT] --frozen requires existing .taida/taida.lock"
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            }
        }
        println!("Generated taida.lock (empty)");
        // Write empty lockfile
        let lockfile = pkg::lockfile::Lockfile::from_resolved(&[]);
        if let Some(parent) = lock_path.parent() {
            // N-56: directory creation error is caught by the subsequent
            // lockfile.write() call, which will report a clear error.
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = lockfile.write(&lock_path) {
            eprintln!("Warning: could not write lockfile: {}", e);
        }
        return;
    }

    println!("Installing dependencies for '{}'...", manifest.name);

    // Read existing lockfile to pin generation-only versions for reproducibility
    let lock_path = project_dir.join(".taida").join("taida.lock");
    let existing_lockfile = match pkg::lockfile::Lockfile::read(&lock_path) {
        Ok(lockfile) => lockfile,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    if frozen && existing_lockfile.is_none() {
        eprintln!("[E32K2_LOCKFILE_DRIFT] --frozen requires existing .taida/taida.lock");
        std::process::exit(1);
    }

    // Resolve all dependencies using the provider chain,
    // pinning generation-only versions to locked exact versions when available.
    // C17-2: forward refresh flags so the StoreProvider can consult the
    // stale-detection decision table (or bypass it for --force-refresh).
    let result = match &existing_lockfile {
        Some(lf) => pkg::resolver::resolve_deps_locked_with_flags(&manifest, lf, refresh_flags),
        None => pkg::resolver::resolve_deps_with_flags(&manifest, refresh_flags),
    };

    // Report errors
    for err in &result.errors {
        eprintln!("  ERROR: {}", err);
    }

    if result.errors.is_empty() {
        // Triple equality (version / source / integrity) is only required
        // under --frozen. Non-frozen install is documented as "generate /
        // update the lockfile", so legitimate drift (version bump in
        // packages.tdm, newly added dep) must rewrite the lockfile rather
        // than fail. Schema malformation is independently caught by
        // `Lockfile::read` -> `parse` ->
        // `validate_schema`, so it remains rejected regardless of frozen.
        if frozen {
            if let Some(lockfile) = &existing_lockfile
                && let Err(e) = lockfile.validate_resolved_bindings(&result.packages)
            {
                eprintln!("{}", e);
                std::process::exit(1);
            }
            if !existing_lockfile
                .as_ref()
                .map(|lf| lf.is_up_to_date(&result.packages))
                .unwrap_or(false)
            {
                eprintln!(
                    "[E32K2_LOCKFILE_DRIFT] --frozen requires .taida/taida.lock to match packages.tdm"
                );
                std::process::exit(1);
            }
        }
    }
    if frozen && !result.errors.is_empty() {
        eprintln!("\nSome dependencies could not be resolved. See errors above.");
        std::process::exit(1);
    }

    // Install resolved dependencies
    let mut addon_map: std::collections::BTreeMap<String, pkg::lockfile::LockedAddon> =
        std::collections::BTreeMap::new();
    if !result.resolved.is_empty() {
        match pkg::resolver::install_deps(&manifest, &result) {
            Ok(()) => {
                for pkg in &result.packages {
                    let source_label = match &pkg.source {
                        pkg::provider::PackageSource::Path(p) => format!("path:{}", p),
                        pkg::provider::PackageSource::CoreBundled => "bundled".to_string(),
                        pkg::provider::PackageSource::Store { org, name } => {
                            format!("github:{}/{}", org, name)
                        }
                    };
                    println!("  {} @{} ({})", pkg.name, pkg.version, source_label);
                }
                println!(
                    "Installed {} package(s) in .taida/deps/",
                    result.packages.len()
                );
            }
            Err(e) => {
                eprintln!("Error installing dependencies: {}", e);
                std::process::exit(1);
            }
        }

        // RC1.5-3a: install addon prebuilds
        // RC2.7-4b: pass allow_local_addon_build for fallback policy
        let existing_lock = pkg::lockfile::Lockfile::read(&lock_path).unwrap_or(None);
        addon_map = match pkg::resolver::install_addon_prebuilds(
            &manifest,
            &result,
            force_refresh,
            existing_lock.as_ref(),
            allow_local_addon_build,
            pkg::resolver::AddonInstallPolicy::from_manifest(&manifest, allow_fresh),
        ) {
            Ok(map) => map,
            Err(e) => {
                eprintln!("Error installing addon prebuilds: {}", e);
                std::process::exit(1);
            }
        };

        if !addon_map.is_empty() {
            for (pkg_name, addon) in &addon_map {
                println!("  Addon {} @ {} ({})", pkg_name, addon.target, addon.sha256);
            }
        }
    }

    if frozen {
        println!("taida.lock is frozen and up to date");
    } else {
        // Generate lockfile (always, even if some deps failed)
        // RC1.5: include addon info if addon prebuilds were installed
        match pkg::resolver::write_lockfile_with_addons(&manifest, &result, &addon_map) {
            Ok(()) => println!("Generated taida.lock"),
            Err(e) => eprintln!("Warning: could not write lockfile: {}", e),
        }
    }

    if !result.errors.is_empty() {
        eprintln!("\nSome dependencies could not be resolved. See errors above.");
        std::process::exit(1);
    }
}

pub(crate) fn run_migrate_lockfile(args: &[String]) {
    for arg in args {
        if is_help_flag(arg.as_str()) {
            println!(
                "\
Usage:
  taida ingot migrate-lockfile

Behavior:
  Rewrite `.taida/taida.lock` from schema v1 / `fnv1a:` integrity to
  the current schema / `sha256:` integrity using the installed `.taida/deps` tree.
  Missing installed dependencies fail with `[E32K2_LOCKFILE_MIGRATE_FAIL]`."
            );
            return;
        }
    }
    if !args.is_empty() {
        eprintln!("Unexpected arguments.");
        eprintln!("Run `taida ingot migrate-lockfile --help` for usage.");
        std::process::exit(1);
    }

    let project_dir = find_packages_tdm().unwrap_or_else(|| {
        eprintln!("No packages.tdm found in current directory or parent directories.");
        eprintln!("Run 'taida init' to create a new project.");
        std::process::exit(1);
    });
    let lock_path = project_dir.join(".taida").join("taida.lock");
    let deps_dir = project_dir.join(".taida").join("deps");

    match pkg::lockfile::Lockfile::migrate_current_from_installed(&lock_path, &deps_dir) {
        Ok(lockfile) => {
            println!(
                "Migrated taida.lock to schema v{} ({} package(s))",
                lockfile.version,
                lockfile.packages.len()
            );
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}

pub(crate) fn run_update(args: &[String]) {
    // Parse --allow-local-addon-build for local addon development.
    let mut allow_local_addon_build = false;
    for arg in args {
        if arg == "--allow-local-addon-build" {
            allow_local_addon_build = true;
        } else if is_help_flag(arg.as_str()) {
            print_update_help();
            return;
        } else {
            eprintln!("Unexpected arguments.");
            eprintln!("Run `taida ingot update --help` for usage.");
            std::process::exit(1);
        }
    }

    // Find project root by looking for packages.tdm
    let project_dir = find_packages_tdm().unwrap_or_else(|| {
        eprintln!("No packages.tdm found in current directory or parent directories.");
        eprintln!("Run 'taida init' to create a new project.");
        std::process::exit(1);
    });

    // Parse manifest
    let manifest = match pkg::manifest::Manifest::from_dir(&project_dir) {
        Ok(Some(m)) => m,
        Ok(None) => {
            eprintln!("No packages.tdm found in '{}'", project_dir.display());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error parsing packages.tdm: {}", e);
            std::process::exit(1);
        }
    };

    if manifest.deps.is_empty() {
        println!("No dependencies defined in packages.tdm");
        return;
    }

    println!("Updating dependencies for '{}'...", manifest.name);

    // Resolve all dependencies with force-remote (bypass local cache for generations)
    let result = pkg::resolver::resolve_deps_update(&manifest);

    // Report errors
    for err in &result.errors {
        eprintln!("  ERROR: {}", err);
    }

    // Install resolved dependencies (recreate symlinks)
    let mut addon_map: std::collections::BTreeMap<String, pkg::lockfile::LockedAddon> =
        std::collections::BTreeMap::new();
    if !result.resolved.is_empty() {
        match pkg::resolver::install_deps(&manifest, &result) {
            Ok(()) => {
                for pkg in &result.packages {
                    let source_label = match &pkg.source {
                        pkg::provider::PackageSource::Path(p) => format!("path:{}", p),
                        pkg::provider::PackageSource::CoreBundled => "bundled".to_string(),
                        pkg::provider::PackageSource::Store { org, name } => {
                            format!("github:{}/{}", org, name)
                        }
                    };
                    println!("  {} @{} ({})", pkg.name, pkg.version, source_label);
                }
                println!(
                    "Updated {} package(s) in .taida/deps/",
                    result.packages.len()
                );
            }
            Err(e) => {
                eprintln!("Error installing dependencies: {}", e);
                std::process::exit(1);
            }
        }

        // Install addon prebuilds after deps are recreated.
        // Without this, `taida ingot update` destroys addon binaries because
        // `install_deps` recreates `.taida/deps` from scratch.
        let lock_path = project_dir.join(".taida").join("taida.lock");
        let existing_lock = pkg::lockfile::Lockfile::read(&lock_path).unwrap_or(None);
        addon_map = match pkg::resolver::install_addon_prebuilds(
            &manifest,
            &result,
            false, // force_refresh: update fetches latest versions but does not bypass cache
            existing_lock.as_ref(),
            allow_local_addon_build,
            pkg::resolver::AddonInstallPolicy::from_manifest(&manifest, false),
        ) {
            Ok(map) => map,
            Err(e) => {
                eprintln!("Error installing addon prebuilds: {}", e);
                std::process::exit(1);
            }
        };

        if !addon_map.is_empty() {
            for (pkg_name, addon) in &addon_map {
                println!("  Addon {} @ {} ({})", pkg_name, addon.target, addon.sha256);
            }
        }
    }

    // Preserve addon stanzas when writing the lockfile.
    // The old write_lockfile call would discard all [[package.addon]] entries.
    match pkg::resolver::write_lockfile_with_addons(&manifest, &result, &addon_map) {
        Ok(()) => println!("Updated taida.lock"),
        Err(e) => eprintln!("Warning: could not write lockfile: {}", e),
    }

    if !result.errors.is_empty() {
        eprintln!("\nSome dependencies could not be resolved. See errors above.");
        std::process::exit(1);
    }
}

#[cfg(feature = "community")]
/// `taida ingot publish` is now a tag-push-only command.
///
/// Flow:
///
/// 1. Find the `packages.tdm` in the current tree and parse it.
/// 2. Validate the manifest identity (`<<<@version owner/name`
/// required; bare names are rejected).
/// 3. Cross-check identity against `origin` (GitHub URL, exact
/// `owner/repo` match).
/// 4. Check the working tree is clean.
/// 5. Compute the next version from the public API diff (or honour
/// `--force-version`).
/// 6. Detect tag collision (reject unless `--retag`).
/// 7. `--dry-run` prints the plan and exits.
/// 8. Otherwise, `git tag <next> && git push origin <tag>`. Done.
///
/// `taida ingot publish` does not build cdylibs, compute SHA-256, mutate
/// `packages.tdm`, push to `main`, or call `gh release create`. All
/// release artefact work is delegated to the addon
/// `release.yml` running as `github-actions[bot]`.
pub(crate) fn run_publish(args: &[String]) {
    // ── CLI parsing ──────────────────────────────────────
    let mut label: Option<String> = None;
    let mut force_version: Option<String> = None;
    let mut retag = false;
    let mut dry_run = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_publish_help();
                return;
            }
            "--label" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Missing value for --label.");
                    eprintln!("Run `taida ingot publish --help` for usage.");
                    std::process::exit(1);
                }
                label = Some(args[i].clone());
            }
            "--force-version" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Missing value for --force-version.");
                    eprintln!("Run `taida ingot publish --help` for usage.");
                    std::process::exit(1);
                }
                force_version = Some(args[i].clone());
            }
            "--retag" => retag = true,
            "--dry-run" => dry_run = true,
            raw if raw.starts_with("--dry-run=") => {
                eprintln!(
                    "`--dry-run=<mode>` was removed in @c.14.rc1. Use plain `--dry-run` instead."
                );
                eprintln!("Run `taida ingot publish --help` for the new flow.");
                std::process::exit(1);
            }
            "--target" => {
                eprintln!(
                    "`--target` was removed in @c.14.rc1. `taida ingot publish` now only pushes a git tag; \
                     addon builds happen in CI via `.github/workflows/release.yml`."
                );
                eprintln!("Run `taida ingot publish --help` for the new flow.");
                std::process::exit(1);
            }
            raw if raw.starts_with('-') => {
                eprintln!("Unknown option for publish: {}", raw);
                eprintln!("Run `taida ingot publish --help` for usage.");
                std::process::exit(1);
            }
            other => {
                eprintln!("Unexpected argument for publish: {}", other);
                eprintln!("Run `taida ingot publish --help` for usage.");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // ── Project discovery ──────────────────────────────────
    let project_dir = find_packages_tdm().unwrap_or_else(|| {
        eprintln!("No packages.tdm found in current directory or parent directories.");
        eprintln!("Run 'taida init' to create a new project.");
        std::process::exit(1);
    });

    let manifest = match pkg::manifest::Manifest::from_dir(&project_dir) {
        Ok(Some(m)) => m,
        Ok(None) => {
            eprintln!("No packages.tdm found in '{}'", project_dir.display());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error parsing packages.tdm: {}", e);
            std::process::exit(1);
        }
    };

    // ── Invariant: working tree must be clean ──────────────
    if let Err(e) = pkg::publish::check_worktree_clean(&project_dir) {
        eprintln!("Publish refused: {}", e);
        std::process::exit(1);
    }

    // ── Plan ───────────────────────────────────────────────
    let plan = match pkg::publish::plan_publish(
        &project_dir,
        &manifest,
        label.as_deref(),
        force_version.as_deref(),
        retag,
    ) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("Publish refused: {}", e);
            std::process::exit(1);
        }
    };

    // ── Dry-run exits after printing the plan ──────────────
    if dry_run {
        print!("{}", pkg::publish::render_plan(&plan));
        return;
    }

    // ── Authentication check (real run only) ───────────────
    if let Err(e) = pkg::publish::check_gh_auth() {
        eprintln!("Publish refused: {}", e);
        std::process::exit(1);
    }

    // ── Tag + push (the only git-mutating step) ────────────
    if let Err(e) = pkg::publish::tag_and_push(&project_dir, &plan.next_version, plan.retag) {
        eprintln!("Publish failed: {}", e);
        std::process::exit(1);
    }

    // ── Report and exit ────────────────────────────────────
    println!(
        "Pushed tag {} for {} to {}.",
        plan.next_version, plan.package_id, plan.remote
    );
    println!("CI (`.github/workflows/release.yml`) will build artefacts and create the release.");
}

pub(crate) fn run_cache(args: &[String]) {
    if args.is_empty() || args.iter().any(|a| is_help_flag(a.as_str())) {
        println!("Usage: taida ingot cache <command> [options]");
        println!();
        println!("Commands:");
        println!("  clean                       Remove cached WASM runtime .o files (default)");
        println!("  clean --addons              Remove cached addon prebuild binaries");
        println!("                              (prunes ~/.taida/addon-cache/)");
        println!("  clean --store [--yes]       Prune ~/.taida/store/ (shows a summary");
        println!("                              first; then asks to confirm interactively on a");
        println!("                              TTY, or requires --yes in non-TTY contexts)");
        println!("  clean --store-pkg <org>/<name>   Prune a single store package");
        println!("                              (no confirmation prompt; scope is narrow)");
        println!("  clean --all [--yes]         Remove WASM + addon cache + store");
        return;
    }

    match args[0].as_str() {
        "clean" => {
            // RC15B-001: parse optional --addons / --all flags.
            // C17-3: add --store / --store-pkg / --yes flags.
            let mut clean_wasm = true;
            let mut clean_addons = false;
            let mut clean_store = false;
            let mut store_pkg: Option<String> = None;
            let mut assume_yes = false;

            let mut i = 1;
            while i < args.len() {
                let extra = args[i].as_str();
                match extra {
                    "--addons" => {
                        clean_wasm = false;
                        clean_addons = true;
                    }
                    "--store" => {
                        clean_wasm = false;
                        clean_store = true;
                    }
                    "--store-pkg" => {
                        clean_wasm = false;
                        i += 1;
                        if i >= args.len() {
                            eprintln!("Missing value for --store-pkg. Expected <org>/<name>.");
                            std::process::exit(1);
                        }
                        store_pkg = Some(args[i].clone());
                    }
                    "--all" => {
                        clean_wasm = true;
                        clean_addons = true;
                        clean_store = true;
                    }
                    "--yes" | "-y" => {
                        assume_yes = true;
                    }
                    other => {
                        eprintln!(
                            "Unknown flag '{}' for 'taida ingot cache clean'. \
                             Use --addons, --store, --store-pkg <org>/<name>, --all, or no flag.",
                            other
                        );
                        std::process::exit(1);
                    }
                }
                i += 1;
            }

            // --store-pkg is mutually exclusive with --store / --all:
            // targeted prune should not also wipe the whole store.
            if store_pkg.is_some() && (clean_store || (clean_wasm && clean_addons)) {
                eprintln!(
                    "--store-pkg cannot be combined with --store or --all. \
                     Use one or the other."
                );
                std::process::exit(1);
            }

            if clean_wasm {
                run_cache_clean();
            }
            if clean_addons {
                run_cache_clean_addons();
            }
            if clean_store {
                run_cache_clean_store(assume_yes);
            }
            if let Some(pkg) = store_pkg {
                run_cache_clean_store_pkg(&pkg);
            }
        }
        other => {
            eprintln!(
                "Unknown cache command '{}'. Use 'taida ingot cache clean'.",
                other
            );
            std::process::exit(1);
        }
    }
}

pub(crate) fn run_cache_clean() {
    // RCB-56: Use absolute CWD to match run_build()'s input_path.parent() behavior.
    // Both now resolve .taida/cache/wasm-rt/ from an absolute path.
    let project_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let cache_dir = codegen::driver::default_wasm_cache_dir(Some(&project_dir));

    if !cache_dir.exists() {
        println!("No cache directory found at '{}'.", cache_dir.display());
        return;
    }

    let mut removed = 0usize;
    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let fname = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
            // Remove cached .o files and temp files, preserve 'include/' dir
            if (fname.ends_with(".o") || fname.ends_with(".tmp.c") || fname.ends_with(".tmp.o"))
                && fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
    }

    if removed > 0 {
        println!(
            "Cleaned {} cached file(s) from '{}'.",
            removed,
            cache_dir.display()
        );
    } else {
        println!(
            "Cache directory '{}' is already clean.",
            cache_dir.display()
        );
    }
}

// RC15B-001: prune the addon prebuild cache at ~/.taida/addon-cache/.
//
// The directory tree is walked by `clean_addon_cache`, which preserves
// user-placed files (anything that is not a recognised addon binary or
// `.manifest-sha256` sidecar) so a confused user can still inspect the
// directory after the command runs.
pub(crate) fn run_cache_clean_addons() {
    match taida::addon::prebuild_fetcher::clean_addon_cache() {
        Ok(summary) => {
            if !summary.root_existed {
                println!("No addon cache found at '{}'.", summary.root.display());
                return;
            }
            let total = summary.binaries_removed + summary.sidecars_removed;
            if total == 0 {
                println!(
                    "Addon cache at '{}' is already clean.",
                    summary.root.display()
                );
            } else {
                let mib = summary.bytes_freed as f64 / (1024.0 * 1024.0);
                println!(
                    "Cleaned {} addon binary file(s) and {} sidecar file(s) ({:.2} MiB) from '{}'.",
                    summary.binaries_removed,
                    summary.sidecars_removed,
                    mib,
                    summary.root.display()
                );
            }
        }
        Err(e) => {
            eprintln!("Error cleaning addon cache: {}", e);
            std::process::exit(1);
        }
    }
}

// C17-3: prune `~/.taida/store/` (all packages, all versions).
//
// Shows a summary first. Requires confirmation (`y` / `yes` / `Y` / `YES`
// on stdin) unless `--yes` is passed. Non-TTY stdin must pass `--yes`
// explicitly so scripts do not wipe the store accidentally.
pub(crate) fn run_cache_clean_store(assume_yes: bool) {
    let store_root = match taida::util::taida_home_dir() {
        Ok(home) => home.join(".taida").join("store"),
        Err(e) => {
            eprintln!("Cannot locate taida home directory: {}", e);
            std::process::exit(1);
        }
    };
    let summary = match taida::pkg::store::summarize_store_root(&store_root) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading store: {}", e);
            std::process::exit(1);
        }
    };
    if !summary.root_existed {
        println!("No store cache found at '{}'.", summary.root.display());
        return;
    }
    if summary.packages_removed == 0 && summary.scratch_removed == 0 {
        println!(
            "Store cache at '{}' is already clean.",
            summary.root.display()
        );
        return;
    }

    // Show summary.
    let mib = summary.bytes_removed as f64 / (1024.0 * 1024.0);
    println!(
        "Store cache at '{}' contains {} package(s), {:.2} MiB.",
        summary.root.display(),
        summary.packages_removed,
        mib
    );
    // C17B-011: report scratch (leftover .tmp-*, .refresh-staging-*)
    // separately so the user sees what is being cleaned up without the
    // count inflating the package number.
    if summary.scratch_removed > 0 {
        println!(
            "  ... and {} leftover scratch directory(ies) from past installs",
            summary.scratch_removed
        );
    }
    // Preview the first few so a user can sanity-check.
    let preview_n = 10usize;
    for name in summary.packages.iter().take(preview_n) {
        println!("  {}", name);
    }
    if summary.packages.len() > preview_n {
        println!("  ... and {} more", summary.packages.len() - preview_n);
    }

    if !assume_yes {
        use std::io::Write;
        let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
        if !is_tty {
            eprintln!(
                "Refusing to prune store in a non-TTY context without --yes. \
                 Re-run with `taida ingot cache clean --store --yes`."
            );
            std::process::exit(1);
        }
        print!("Remove all {} package(s)? [y/N] ", summary.packages_removed);
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).is_err() {
            eprintln!("No input received; aborting.");
            std::process::exit(1);
        }
        let answer = answer.trim();
        if !matches!(answer, "y" | "Y" | "yes" | "YES") {
            println!("Aborted.");
            return;
        }
    }

    match taida::pkg::store::prune_store_root(&store_root) {
        Ok(report) => {
            let mib = report.bytes_removed as f64 / (1024.0 * 1024.0);
            println!(
                "Removed {} package(s) ({:.2} MiB) from '{}'.",
                report.packages_removed,
                mib,
                report.root.display()
            );
        }
        Err(e) => {
            eprintln!("Error pruning store: {}", e);
            std::process::exit(1);
        }
    }
}

// C17-3: prune a single package from the store (all versions of
// `<org>/<name>/*`). No confirmation is required since the scope is
// narrow.
pub(crate) fn run_cache_clean_store_pkg(pkg_spec: &str) {
    let (org, name) = match pkg_spec.split_once('/') {
        Some((o, n)) if !o.is_empty() && !n.is_empty() && !n.contains('/') => (o, n),
        _ => {
            eprintln!(
                "Invalid --store-pkg value '{}'. Expected <org>/<name>.",
                pkg_spec
            );
            std::process::exit(1);
        }
    };
    let store_root = match taida::util::taida_home_dir() {
        Ok(home) => home.join(".taida").join("store"),
        Err(e) => {
            eprintln!("Cannot locate taida home directory: {}", e);
            std::process::exit(1);
        }
    };
    match taida::pkg::store::prune_store_package(&store_root, org, name) {
        Ok(report) => {
            if !report.root_existed {
                println!("No store cache found at '{}'.", report.root.display());
                return;
            }
            if report.packages_removed == 0 {
                println!(
                    "Package '{}/{}' not found in store at '{}'.",
                    org,
                    name,
                    report.root.display()
                );
                return;
            }
            let mib = report.bytes_removed as f64 / (1024.0 * 1024.0);
            println!(
                "Removed {} version(s) of {}/{} ({:.2} MiB) from '{}'.",
                report.packages_removed,
                org,
                name,
                mib,
                report.root.display()
            );
        }
        Err(e) => {
            eprintln!("Error pruning store package: {}", e);
            std::process::exit(1);
        }
    }
}

pub(crate) fn find_packages_tdm_from(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        if dir.join("packages.tdm").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

pub(crate) fn find_packages_tdm() -> Option<PathBuf> {
    let dir = env::current_dir().ok()?;
    find_packages_tdm_from(&dir)
}
