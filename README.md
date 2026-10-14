# Shorts Factory Desktop

**Make, edit and publish vertical Reels on your PC — made with [FilmCraft](https://github.com/storytold/filmcraft).**

Shorts Factory Desktop is a native desktop app for channel-branded Instagram Reels and YouTube
Shorts: pick a workspace (channel), bring footage, let your local AI write the card, lay it out with
one of 21 channel templates, cut it on a professional timeline, export, and publish — directly from
the desktop, or through the optional Supabase backend so posts go out while the PC is off.

It is an independent fork of **FilmCraft**, the open-source, pure-Rust non-linear video editor by
the ArtCraft Team and the FilmCraft contributors. Everything FilmCraft does — source and program
monitors, a frame-exact timeline with every trim mode, keyframes, effects and transitions, colour,
audio mixing, titles and its own codecs — is here, with Reel making built on top.

## What Shorts Factory adds

| | |
|---|---|
| **Reel Studio panel** | Workspace (channel) picker, the template gallery, card fields, AI writing, export to the library, approval, publishing and scheduling — in one docked panel. |
| **21 Reel templates** | Quote card, Full bleed, Headline hero, Numbered point, Framed, Brief, Logo only and fourteen JKR designs, rebuilt as ordinary FilmCraft clips (blurred backdrop, card, footage fitted to the slot, overlay graphic, logo) — every part stays editable on the timeline. |
| **Your existing library, in place** | Reads the Shorts Factory data folder (`%APPDATA%\Shorts Factory Desktop`): workspaces, branding, media, articles, posts, renders, publishing accounts and their keys. Nothing is copied or re-entered; the earlier app keeps working alongside. |
| **Local AI cards** | Hermes or Ollama (whichever the library is set to) writes the headline, caption, attribution, Instagram description and hashtags in the channel's voice. |
| **Publishing** | Instagram Reels (resumable upload, reconciliation, no double posting) and YouTube, from the desktop; scheduling hands off to the earlier app's worker or runs here. |

Engine commands for all of it (`reel.*`) are agent-drivable like every FilmCraft command.

See [docs/SHORTS-FACTORY.md](docs/SHORTS-FACTORY.md) for the feature parity checklist with the
earlier Electron app and what is still in progress.

## Build and run

Windows x64 with Rust 1.95+ (rustup, MSVC toolchain):

```powershell
cargo run --release -p filmcraft --bin shorts-factory-desktop
```

The executable is `target\release\shorts-factory-desktop.exe`. Editor settings and autosaves live in
`%APPDATA%\Shorts Factory Desktop\Editor`; the library stays where it is.

## Credits and license

Made with FilmCraft — Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors.
Shorts Factory Desktop additions — Copyright (c) 2026 the Shorts Factory Desktop contributors.
Licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE), at your option. See
[NOTICE](NOTICE) and [ATTRIBUTION.md](ATTRIBUTION.md).

FilmCraft and ArtCraft are trademarks of the ArtCraft Team. This fork is not an ArtCraft product and
does not use their wordmark or logos. FilmCraft's own README is kept at
[docs/FILMCRAFT-README.md](docs/FILMCRAFT-README.md); upstream is the `upstream` git remote.
