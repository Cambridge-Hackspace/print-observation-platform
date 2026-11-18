//! A Bambu printer's camera on port 6000 (#33): the decoder fuzzed against
//! what a broken or hostile camera could send, and the client against the
//! fake camera.
//!
//! The fuzzer is seeded: it prints its seed, and `POP_FUZZ_SEED=<n>` replays
//! a run exactly. `POP_FUZZ_CASES` sets how many streams it tries.

use std::{sync::Arc, time::Duration};

use pop_printers::{
    bambu::{MemoryPins, PinStore},
    bambu_camera::{BambuCamera, CameraError, Decoder, FrameError, HEADER, MAX_PICTURE, sign_in},
    fakes::bambu_camera::{FakeBambuCamera, JPEG, Sends, framed},
};

// ---- the decoder ---------------------------------------------------------------

/// xorshift64*: enough randomness to fuzz with, and replayable from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

fn seed() -> u64 {
    let s = std::env::var("POP_FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64
                | 1
        });
    eprintln!("SEED={s} (replay with POP_FUZZ_SEED={s})");
    s
}

/// A JPEG of `n` bytes: the magic at each end, anything between.
fn jpeg(rng: &mut Rng, n: usize) -> Vec<u8> {
    let n = n.max(4);
    let mut j = rng.bytes(n);
    j[0] = 0xff;
    j[1] = 0xd8;
    j[n - 2] = 0xff;
    j[n - 1] = 0xd9;
    j
}

fn header(size: u32) -> Vec<u8> {
    let mut h = size.to_le_bytes().to_vec();
    h.extend_from_slice(&[0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
    h
}

/// Feed `stream` in random pieces, checking after every one what the
/// decoder holds and what it gives. Returns the pictures and the first error.
fn feed(rng: &mut Rng, d: &mut Decoder, stream: &[u8]) -> (Vec<Vec<u8>>, Option<FrameError>) {
    let mut out = Vec::new();
    let mut rest = stream;
    while !rest.is_empty() {
        // Mostly small pieces, sometimes large ones.
        let n = if rng.below(4) == 0 {
            1 + rng.below(1 << 20)
        } else {
            1 + rng.below(5000)
        };
        let (piece, r) = rest.split_at(n.min(rest.len()));
        rest = r;
        let got = d.push(piece);
        assert!(
            d.held() <= HEADER + MAX_PICTURE,
            "the decoder holds {} bytes",
            d.held()
        );
        match got {
            Ok(pictures) => {
                for p in &pictures {
                    assert!(p.len() <= MAX_PICTURE);
                    assert!(p.starts_with(&[0xff, 0xd8]) && p.ends_with(&[0xff, 0xd9]));
                }
                out.extend(pictures);
            }
            Err(e) => {
                assert_eq!(d.held(), 0, "an error resets the decoder");
                return (out, Some(e));
            }
        }
    }
    (out, None)
}

#[test]
fn whole_pictures_come_out_however_the_stream_is_split() {
    let mut rng = Rng(7);
    let pictures: Vec<Vec<u8>> = [22, 1, 4096, 4097, 9000]
        .iter()
        .map(|n| jpeg(&mut rng, *n))
        .collect();
    let stream: Vec<u8> = pictures.iter().flat_map(|p| framed(p)).collect();
    for split in [1, 2, 3, 15, 16, 17, 4095, 4096, 4113, stream.len()] {
        let mut d = Decoder::new();
        let mut got = Vec::new();
        for piece in stream.chunks(split) {
            got.extend(d.push(piece).unwrap());
        }
        assert_eq!(got, pictures, "split into {split}-byte pieces");
        assert_eq!(d.held(), 0);
    }
}

#[test]
fn a_header_that_claims_too_much_is_refused_before_anything_is_held() {
    let mut d = Decoder::new();
    let size = u32::try_from(MAX_PICTURE + 1).unwrap();
    assert_eq!(
        d.push(&header(size)),
        Err(FrameError::TooLarge(MAX_PICTURE + 1))
    );
    assert_eq!(d.held(), 0);
    let mut d = Decoder::new();
    assert_eq!(
        d.push(&header(u32::MAX)),
        Err(FrameError::TooLarge(u32::MAX as usize))
    );
    // The largest picture allowed is taken.
    let mut rng = Rng(3);
    let big = jpeg(&mut rng, MAX_PICTURE);
    let mut d = Decoder::new();
    assert_eq!(d.push(&framed(&big)).unwrap(), vec![big]);
}

#[test]
fn an_empty_picture_and_one_that_is_not_a_jpeg_are_refused() {
    let mut d = Decoder::new();
    assert_eq!(d.push(&header(0)), Err(FrameError::Empty));
    let mut d = Decoder::new();
    assert_eq!(d.push(&framed(b"not a picture")), Err(FrameError::NotJpeg));
    // The start alone is not enough; nor the end alone.
    let mut d = Decoder::new();
    assert_eq!(
        d.push(&framed(&[0xff, 0xd8, 0, 0])),
        Err(FrameError::NotJpeg)
    );
    let mut d = Decoder::new();
    assert_eq!(
        d.push(&framed(&[0, 0, 0xff, 0xd9])),
        Err(FrameError::NotJpeg)
    );
}

/// Streams built from what a broken or hostile camera could send: good
/// pictures, headers claiming any size, bytes that are not pictures, bits
/// flipped in good streams, and streams cut short. None panics; none makes
/// the decoder hold more than a picture's worth; every picture it gives is a
/// whole JPEG.
#[test]
fn the_decoder_survives_whatever_a_camera_sends() {
    let seed = seed();
    let cases: usize = std::env::var("POP_FUZZ_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(400);
    let mut rng = Rng(seed);
    let mut pictures_seen = 0;
    let mut errors_seen = 0;
    for _ in 0..cases {
        let mut stream = Vec::new();
        for _ in 0..1 + rng.below(6) {
            match rng.below(6) {
                0 | 1 => {
                    let n = 4 + rng.below(20_000);
                    stream.extend(framed(&jpeg(&mut rng, n)));
                }
                2 => {
                    // A header claiming any size, then some bytes.
                    let size = match rng.below(3) {
                        0 => rng.next() as u32,
                        1 => u32::try_from(MAX_PICTURE).unwrap() + rng.below(1000) as u32,
                        _ => rng.below(100) as u32,
                    };
                    stream.extend(header(size));
                    let n = rng.below(1 << 21);
                    stream.extend(rng.bytes(n));
                }
                3 => {
                    let n = rng.below(5000);
                    stream.extend(rng.bytes(n));
                }
                4 => {
                    let n = 4 + rng.below(5000);
                    let mut f = framed(&jpeg(&mut rng, n));
                    for _ in 0..1 + rng.below(4) {
                        let i = rng.below(f.len());
                        f[i] ^= 1 << rng.below(8);
                    }
                    stream.extend(f);
                }
                _ => {
                    let n = 4 + rng.below(5000);
                    let f = framed(&jpeg(&mut rng, n));
                    let cut = rng.below(f.len());
                    stream.extend(&f[..cut]);
                }
            }
        }
        let mut d = Decoder::new();
        let (pictures, error) = feed(&mut rng, &mut d, &stream);
        pictures_seen += pictures.len();
        errors_seen += usize::from(error.is_some());
    }
    // The fuzzer reached both sides: it is not passing on streams that were
    // all good, or all refused.
    assert!(
        pictures_seen > 0 && errors_seen > 0,
        "{pictures_seen} pictures, {errors_seen} errors"
    );
}

#[test]
fn signing_in_is_the_eighty_bytes_the_sources_describe() {
    // Written out by hand from OpenBambuAPI's table, not from the code.
    let mut want = vec![0x40, 0, 0, 0, 0x00, 0x30, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    want.extend_from_slice(b"bblp");
    want.extend_from_slice(&[0; 28]);
    want.extend_from_slice(b"12345678");
    want.extend_from_slice(&[0; 24]);
    assert_eq!(sign_in("12345678").unwrap().to_vec(), want);
    assert!(sign_in(&"x".repeat(33)).is_err());
    assert!(sign_in("café").is_err());
}

// ---- the client, against the fake camera ----------------------------------------

const SERIAL: &str = "01P00A123456789";
const CODE: &str = "12345678";

fn camera(fake: &FakeBambuCamera, code: &str, pins: Arc<dyn PinStore>) -> BambuCamera {
    BambuCamera {
        host: fake.host(),
        port: fake.port(),
        serial: SERIAL.into(),
        access_code: code.into(),
        pins,
    }
}

#[tokio::test]
async fn pictures_come_from_the_camera_and_its_certificate_is_pinned() {
    let fake = FakeBambuCamera::start(SERIAL, CODE, Duration::from_millis(20)).await;
    fake.split(7);
    let pins = Arc::new(MemoryPins::default());
    let mut p = camera(&fake, CODE, pins.clone()).connect().await.unwrap();
    for _ in 0..3 {
        assert_eq!(p.next().await.unwrap(), JPEG);
    }
    assert_eq!(fake.signed_in(), 1);
    assert_eq!(
        pins.pinned(&BambuCamera::pin_key(SERIAL)),
        Some(fake.fingerprint())
    );
    assert_eq!(
        pins.pinned(SERIAL),
        None,
        "the MQTT pin is the driver's, not the camera's"
    );
}

#[tokio::test]
async fn a_wrong_access_code_is_said_as_such() {
    let fake = FakeBambuCamera::start(SERIAL, CODE, Duration::from_millis(20)).await;
    let mut p = camera(&fake, "87654321", Arc::new(MemoryPins::default()))
        .connect()
        .await
        .unwrap();
    let e = p.next().await.unwrap_err();
    assert!(matches!(e, CameraError::Refused(_)), "{e:?}");
    assert!(e.to_string().contains("access code"), "{e}");
    assert_eq!(fake.signed_in(), 0);
}

#[tokio::test]
async fn a_different_certificate_is_refused_and_named() {
    let fake = FakeBambuCamera::start(SERIAL, CODE, Duration::from_millis(20)).await;
    let pins = Arc::new(MemoryPins::default());
    let mut p = camera(&fake, CODE, pins.clone()).connect().await.unwrap();
    p.next().await.unwrap();
    fake.new_certificate(SERIAL);
    match camera(&fake, CODE, pins.clone()).connect().await {
        Err(CameraError::Certificate { seen }) => assert_eq!(seen, fake.fingerprint()),
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("a different certificate was taken"),
    }
}

#[tokio::test]
async fn a_stream_that_goes_wrong_ends_with_why() {
    let fake = FakeBambuCamera::start(SERIAL, CODE, Duration::from_millis(20)).await;
    let pins: Arc<dyn PinStore> = Arc::new(MemoryPins::default());
    fake.sends(Sends::TooLarge(u32::MAX));
    let mut p = camera(&fake, CODE, pins.clone()).connect().await.unwrap();
    let e = tokio::time::timeout(Duration::from_secs(5), p.next())
        .await
        .expect("refused at the header, not after reading what it claims")
        .unwrap_err();
    assert_eq!(
        e,
        CameraError::Stream(FrameError::TooLarge(u32::MAX as usize))
    );

    fake.sends(Sends::NotJpeg);
    let mut p = camera(&fake, CODE, pins.clone()).connect().await.unwrap();
    assert_eq!(
        p.next().await.unwrap_err(),
        CameraError::Stream(FrameError::NotJpeg)
    );

    fake.sends(Sends::CutOff);
    let mut p = camera(&fake, CODE, pins.clone()).connect().await.unwrap();
    let e = p.next().await.unwrap_err();
    assert!(matches!(e, CameraError::Unreachable(_)), "{e:?}");
}

#[tokio::test]
async fn an_access_code_no_printer_gives_is_refused_before_connecting() {
    let fake = FakeBambuCamera::start(SERIAL, CODE, Duration::from_millis(20)).await;
    let e = camera(&fake, &"9".repeat(33), Arc::new(MemoryPins::default()))
        .connect()
        .await
        .err()
        .unwrap();
    assert!(matches!(e, CameraError::Refused(_)), "{e:?}");
    assert_eq!(fake.signed_in(), 0);
}

// ---- the RTSPS relay (X1, P2S, H2) ---------------------------------------------

use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn ask_through(port: u16, request: &str) -> String {
    let mut local = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    local.write_all(request.as_bytes()).await.unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 1024];
    while !got.windows(4).any(|w| w == b"\r\n\r\n") {
        match tokio::time::timeout(Duration::from_secs(5), local.read(&mut buf)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(n)) => got.extend_from_slice(&buf[..n]),
            Ok(Err(_)) => break,
        }
    }
    String::from_utf8_lossy(&got).into_owned()
}

const OPTIONS: &str = "OPTIONS rtsp://127.0.0.1/streaming/live/1 RTSP/1.0\r\nCSeq: 1\r\n\r\n";

#[tokio::test]
async fn the_relay_passes_rtsp_both_ways_with_the_certificate_pinned() {
    let fake = FakeBambuCamera::start_rtsps(SERIAL).await;
    let pins = Arc::new(MemoryPins::default());
    let relay = camera(&fake, CODE, pins.clone()).relay().await.unwrap();
    let reply = ask_through(relay.port, OPTIONS).await;
    assert!(
        reply.starts_with("RTSP/1.0 200 OK\r\nCSeq: 1\r\n"),
        "{reply:?}"
    );
    assert_eq!(
        fake.rtsp_requests(),
        ["OPTIONS rtsp://127.0.0.1/streaming/live/1 RTSP/1.0"]
    );
    assert_eq!(
        pins.pinned(&BambuCamera::pin_key(SERIAL)),
        Some(fake.fingerprint())
    );
    assert_eq!(relay.last_error(), None);
}

#[tokio::test]
async fn the_relay_passes_nothing_to_a_camera_with_another_certificate() {
    let fake = FakeBambuCamera::start_rtsps(SERIAL).await;
    let pins = Arc::new(MemoryPins::default());
    let relay = camera(&fake, CODE, pins.clone()).relay().await.unwrap();
    assert!(
        ask_through(relay.port, OPTIONS)
            .await
            .starts_with("RTSP/1.0 200")
    );
    fake.new_certificate(SERIAL);
    let reply = ask_through(relay.port, OPTIONS).await;
    assert_eq!(reply, "", "nothing came back");
    assert_eq!(
        fake.rtsp_requests().len(),
        1,
        "and nothing reached the camera"
    );
    assert_eq!(
        relay.last_error(),
        Some(CameraError::Certificate {
            seen: fake.fingerprint()
        })
    );
}
