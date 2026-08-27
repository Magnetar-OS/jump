// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-only

//! Localisation, shared by all three binaries.
//!
//! The standard COSMIC arrangement: `i18n-embed` with the Fluent backend, the
//! catalogue compiled in through `rust-embed`, and a crate-local `fl!` macro.
//! The macro is byte-identical across cosmic-launcher, cosmic-app-library and
//! cosmic-edit — copied, not depended on — and this is the same file, living in
//! the library crate so the launcher, the applet and the settings window share
//! one loader instead of carrying three.
//!
//! `fl!` is compile-time checked against `i18n/en/jump.ftl`: a typo in a
//! message id is a build error, not a label reading `some-id` at runtime.

use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::{DefaultLocalizer, LanguageLoader, Localizer};
use rust_embed::RustEmbed;
use std::sync::LazyLock;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader: FluentLanguageLoader = fluent_language_loader!();

    loader
        .load_fallback_language(&Localizations)
        .expect("error while loading fallback language");

    loader
});

#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::localize::LANGUAGE_LOADER, $message_id)
    }};

    ($message_id:literal, $($args:expr),*) => {{
        i18n_embed_fl::fl!($crate::localize::LANGUAGE_LOADER, $message_id, $($args), *)
    }};
}

/// Get the [`Localizer`] to be used for localizing this library.
pub fn localizer() -> Box<dyn Localizer> {
    Box::from(DefaultLocalizer::new(&*LANGUAGE_LOADER, &Localizations))
}

/// Apply the system's requested languages.
///
/// Called from `main` in each binary before the application is constructed,
/// because `fl!` is evaluated during `init` when the first widgets are built.
pub fn localize() {
    let localizer = localizer();
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();

    if let Err(error) = localizer.select(&requested_languages) {
        eprintln!("error while loading fluent localizations: {error}");
    }
}
