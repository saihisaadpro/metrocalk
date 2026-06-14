//! `dd-capture` — swapchain-aware screen capture via **DXGI Desktop Duplication**.
//!
//! Why this exists: M2.1's 1b used GDI `CopyFromScreen`, which **cannot see a GPU flip-model /
//! overlay swapchain** — so a live wgpu viewport reads as all-black even when it's rendering. Desktop
//! Duplication grabs the **DWM's final composited output** (the literal pixels on screen, overlays
//! included), so it sees the wgpu layer through the transparent WebView2. That makes the
//! blackout-vs-real-composite question answerable by automation, holding to the prompt's higher bar.
//!
//! Usage: `dd-capture --out frame.bmp [--rect x,y,w,h]`. Captures one composited frame from the
//! primary output, optionally crops to a window rect, writes a 32-bit BMP, and prints a one-line JSON
//! analysis (black %, distinct colors, viewport-clear presence, bright-UI presence).

use std::fs::File;
use std::io::Write;

use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIOutput1, IDXGIResource, DXGI_OUTDUPL_FRAME_INFO,
};

struct Args {
    out: String,
    rect: Option<(i32, i32, i32, i32)>,
}

fn parse_args() -> Args {
    let mut out = "capture.bmp".to_string();
    let mut rect = None;
    let argv: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--out" => {
                i += 1;
                out = argv.get(i).cloned().unwrap_or(out);
            }
            "--rect" => {
                i += 1;
                if let Some(s) = argv.get(i) {
                    let p: Vec<i32> = s.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                    if p.len() == 4 {
                        rect = Some((p[0], p[1], p[2], p[3]));
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    Args { out, rect }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("dd-capture error: {e:?}");
        std::process::exit(2);
    }
}

fn run() -> windows::core::Result<()> {
    let args = parse_args();
    let (full_w, full_h, bgra, pitch) = capture_primary()?;

    // Crop (clamped) to the requested rect, else full frame.
    let (cx, cy, cw, ch) = match args.rect {
        Some((x, y, w, h)) => {
            let x = x.clamp(0, full_w as i32);
            let y = y.clamp(0, full_h as i32);
            let w = w.min(full_w as i32 - x).max(0);
            let h = h.min(full_h as i32 - y).max(0);
            (x as usize, y as usize, w as usize, h as usize)
        }
        None => (0, 0, full_w as usize, full_h as usize),
    };

    // Extract the crop as tightly-packed BGRA.
    let mut crop = Vec::with_capacity(cw * ch * 4);
    for row in 0..ch {
        let src = (cy + row) * pitch + cx * 4;
        crop.extend_from_slice(&bgra[src..src + cw * 4]);
    }

    write_bmp(&args.out, cw as i32, ch as i32, &crop)?;
    let a = analyze(&crop, cw, ch);
    println!(
        "{{\"out\":\"{}\",\"full\":[{},{}],\"crop\":[{},{},{},{}],\"black_pct\":{:.1},\"distinct\":{},\"viewport_clear_pct\":{:.1},\"bright_ui_pct\":{:.1},\"mean_rgb\":[{},{},{}]}}",
        args.out, full_w, full_h, cx, cy, cw, ch,
        a.black_pct, a.distinct, a.viewport_pct, a.bright_pct, a.mean_r, a.mean_g, a.mean_b
    );
    Ok(())
}

/// Capture one composited frame from the primary output. Returns (width, height, BGRA bytes, pitch).
fn capture_primary() -> windows::core::Result<(u32, u32, Vec<u8>, usize)> {
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut D3D_FEATURE_LEVEL::default()),
            Some(&mut context),
        )?;
        let device = device.unwrap();
        let context = context.unwrap();

        let dxgi_device: IDXGIDevice = device.cast()?;
        let adapter = dxgi_device.GetAdapter()?;
        let output = adapter.EnumOutputs(0)?;
        let output1: IDXGIOutput1 = output.cast()?;
        let dupl = output1.DuplicateOutput(&device)?;

        // The first AcquireNextFrame after duplicating often reports no new frame until the desktop
        // updates; the composite window animates, so retry until we get a real frame.
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        let mut tries = 0;
        loop {
            let _ = dupl.ReleaseFrame(); // harmless if no frame held
            match dupl.AcquireNextFrame(1000, &mut frame_info, &mut resource) {
                Ok(()) => {
                    if resource.is_some() && frame_info.LastPresentTime != 0 {
                        break;
                    }
                    // metadata-only frame (cursor move); release and retry for a presented frame
                    resource = None;
                    tries += 1;
                }
                Err(e) => {
                    tries += 1;
                    if tries > 40 {
                        return Err(e);
                    }
                }
            }
            if tries > 40 {
                break;
            }
        }
        let resource = resource.expect("no desktop frame acquired");
        let frame_tex: ID3D11Texture2D = resource.cast()?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        frame_tex.GetDesc(&mut desc);
        let (w, h) = (desc.Width, desc.Height);

        // CPU-readable staging copy.
        let mut staging_desc = desc;
        staging_desc.Usage = D3D11_USAGE_STAGING;
        staging_desc.BindFlags = 0;
        staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        staging_desc.MiscFlags = 0;
        let mut staging: Option<ID3D11Texture2D> = None;
        device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
        let staging = staging.unwrap();

        context.CopyResource(&staging, &frame_tex);

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let pitch = mapped.RowPitch as usize;
        let mut bytes = vec![0u8; pitch * h as usize];
        std::ptr::copy_nonoverlapping(mapped.pData as *const u8, bytes.as_mut_ptr(), bytes.len());
        context.Unmap(&staging, 0);
        let _ = dupl.ReleaseFrame();

        Ok((w, h, bytes, pitch))
    }
}

struct Analysis {
    black_pct: f64,
    distinct: usize,
    viewport_pct: f64,
    bright_pct: f64,
    mean_r: u32,
    mean_g: u32,
    mean_b: u32,
}

/// BGRA crop analysis. The composite viewport clear is dark blue (~RGB 10,26,51); "bright UI" is any
/// light pixel (the WebView2 panels/text). Distinct colors on a coarse grid distinguishes a real
/// composite (many) from a flat blackout (≈1).
fn analyze(bgra: &[u8], w: usize, h: usize) -> Analysis {
    let n = (w * h).max(1);
    let mut black = 0usize;
    let mut viewport = 0usize;
    let mut bright = 0usize;
    let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
    let mut seen = std::collections::HashSet::new();
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let b = bgra[i] as i32;
            let g = bgra[i + 1] as i32;
            let r = bgra[i + 2] as i32;
            sr += r as u64;
            sg += g as u64;
            sb += b as u64;
            if r.max(g).max(b) < 12 {
                black += 1;
            }
            // dark-blue viewport clear: blue dominant, all low-ish
            if b > 30 && b < 90 && r < 40 && g < 60 && b > r {
                viewport += 1;
            }
            if r.min(g).min(b) > 140 {
                bright += 1;
            }
            if x % 16 == 0 && y % 16 == 0 {
                seen.insert((r / 8, g / 8, b / 8));
            }
        }
    }
    Analysis {
        black_pct: 100.0 * black as f64 / n as f64,
        distinct: seen.len(),
        viewport_pct: 100.0 * viewport as f64 / n as f64,
        bright_pct: 100.0 * bright as f64 / n as f64,
        mean_r: (sr / n as u64) as u32,
        mean_g: (sg / n as u64) as u32,
        mean_b: (sb / n as u64) as u32,
    }
}

/// Write a top-down 32-bit BGRA BMP (negative height = top-down, no row flip needed).
fn write_bmp(path: &str, w: i32, h: i32, bgra: &[u8]) -> windows::core::Result<()> {
    let mut f = File::create(path).map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), format!("{e}")))?;
    let img_size = (w * h * 4) as u32;
    let file_size = 14 + 40 + img_size;
    let mut hdr = Vec::with_capacity(54);
    hdr.extend_from_slice(b"BM");
    hdr.extend_from_slice(&file_size.to_le_bytes());
    hdr.extend_from_slice(&0u32.to_le_bytes()); // reserved
    hdr.extend_from_slice(&54u32.to_le_bytes()); // pixel data offset
    hdr.extend_from_slice(&40u32.to_le_bytes()); // DIB header size
    hdr.extend_from_slice(&w.to_le_bytes());
    hdr.extend_from_slice(&(-h).to_le_bytes()); // top-down
    hdr.extend_from_slice(&1u16.to_le_bytes()); // planes
    hdr.extend_from_slice(&32u16.to_le_bytes()); // bpp
    hdr.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    hdr.extend_from_slice(&img_size.to_le_bytes());
    hdr.extend_from_slice(&[0u8; 16]); // ppm + palette (unused)
    f.write_all(&hdr).ok();
    f.write_all(bgra).ok();
    Ok(())
}
