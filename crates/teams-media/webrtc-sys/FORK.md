# Fork de LiveKit `webrtc-sys` / `libwebrtc`

Sources : [livekit/rust-sdks](https://github.com/livekit/rust-sdks), crates publiées `webrtc-sys` 0.3.55 et
`libwebrtc` 0.4.2 (Apache-2.0, voir `NOTICE.md`). Binaires libwebrtc précompilés : tag `webrtc-a545de4`,
téléchargés par `webrtc-sys-build` au premier build (`LK_CUSTOM_WEBRTC` pointe vers une build maison).

Les noms de paquets restent ceux d'upstream : cxx dérive le préfixe des en-têtes générés
(`webrtc-sys/src/*.rs.h`) du nom du paquet.

## Ajouts (marqués `Teams fork` dans le code)

| Où | Quoi | Pourquoi |
| --- | --- | --- |
| `src/peer_connection.cpp` | `max-bundle`, `rtcp-mux require`, unified plan forcés | Les relais Teams n'acceptent qu'un transport groupé |
| `src/peer_connection.rs`, `libwebrtc` | `local_description()` (pending ou courante) | Envoyer l'offre une fois le gathering fini, sans trickle |
| `src/rtp_receiver.*`, `libwebrtc` | `get_sources()` → `RtpSource` | CSRC du mix audio : qui parle |
| `src/rtp_sender.*`, `libwebrtc` | `generate_key_frame()` | Keyframe demandée par le mixeur (`controlVideoStreaming`) |
| `libwebrtc/src/rtp_transceiver.rs` | `set_direction()` (déjà dans le pont) | Caméra et partage on/off en cours d'appel |
| `include/livekit/mic_tap.h`, `src/mic_tap.*` | Post-traitement APM vers Rust | Micro après AEC/NS pour l'enregistrement local |
| `src/peer_connection_factory.cpp` | Branche le `mic_tap` sur l'APM | idem |
| `libwebrtc/src/video_frame.rs` | `to_i420()` public | Convertir les frames natives pour l'app |
| `include/livekit/camera_mac.h`, `src/camera_mac.*`, `libwebrtc` `camera_mac` | Caméra macOS via `RTCCameraVideoCapturer` (CVPixelBuffer sans copie) | nokhwa n'ouvre pas la caméra intégrée sous macOS |
| `libwebrtc` `NativeVideoSource::new_external` | Source sans images noires de maintien | Sinon elles s'intercalent avec celles de la caméra native |
| `src/rtp_sender.cpp` | `set_track(nullptr)` accepté | Couper caméra/partage plantait (déréférencement nul) |
| `build.rs` (Linux) | Lie `glib-2.0`, `gobject-2.0`, `gio-2.0` | Le portail XDG du partage d'écran les appelle |

## Retraits

- `frame_cryptor` (E2EE LiveKit), `audio_mixer`, `apm` autonome, `audio_resampler` : inutiles (l'ADM natif
  fait le traitement audio).
- Backend wasm de `libwebrtc`, tests C++ d'upstream.

Restent, trop imbriqués pour être retirés sans risque : data channels (`PeerConnection`, stats) et
packet trailer (sources/pistes vidéo). Les sources Android, Jetson et NVIDIA ne sont compilées que sur
ces cibles.

## Contraintes de build

- macOS : le binaire final doit passer `-ObjC` au linker (cargo ne le propage pas) : `crates/teams-media/build.rs`,
  `crates/teams-core/build.rs`, `apps/macos/Package.swift`.
- Linux : clang ≥ 21 (libc++ hermétique de libwebrtc), en-têtes X11/DRM/GBM/GLib (voir `.github/workflows/media.yml`).
- Windows : MSVC.
- Un seul libwebrtc par processus : lié avec un autre (ex. `LiveKitWebRTC.framework`), les symboles C++
  se mélangent et la création de la factory se bloque.

## Mettre à jour

1. Récupérer les nouvelles versions publiées des deux crates.
2. Réappliquer les lignes du tableau ci-dessus (chercher `Teams fork`) et les retraits.
3. `cargo test -p teams-media` sur les trois systèmes (CI `Media`), puis un appel test au bot d'écho.
