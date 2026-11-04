//! Painting helpers the egui painter doesn't have out of the box: cover-cropped images,
//! gradients, glow and initials tiles.

use egui::{
    epaint::Mesh, load::SizedTexture, pos2, vec2, Color32, CornerRadius, FontFamily, FontId, Painter,
    Pos2, Rect, SizeHint, TextureFilter, TextureOptions, TextureWrapMode, Ui,
};

pub const TEX: TextureOptions = TextureOptions {
    magnification: TextureFilter::Linear,
    minification: TextureFilter::Linear,
    wrap_mode: TextureWrapMode::ClampToEdge,
    mipmap_mode: Some(TextureFilter::Linear),
};

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

/// Returns the texture once it has finished loading (URLs load in the background).
pub fn texture(ctx: &egui::Context, uri: &str) -> Option<SizedTexture> {
    match ctx.try_load_texture(uri, TEX, SizeHint::default()) {
        Ok(egui::load::TexturePoll::Ready { texture }) => Some(texture),
        _ => None,
    }
}

/// UV rect that crops a texture to fill `rect` without stretching (CSS `object-fit: cover`).
/// `focus` picks which part survives the crop: (0.5, 0.5) is the centre.
pub fn cover_uv(tex: Vec2Like, rect: Rect, focus: (f32, f32)) -> Rect {
    let (tw, th) = (tex.0.max(1.0), tex.1.max(1.0));
    let ta = tw / th;
    let ra = rect.width().max(1.0) / rect.height().max(1.0);
    if ta > ra {
        let w = ra / ta;
        let x0 = (1.0 - w) * focus.0;
        Rect::from_min_max(pos2(x0, 0.0), pos2(x0 + w, 1.0))
    } else {
        let h = ta / ra;
        let y0 = (1.0 - h) * focus.1;
        Rect::from_min_max(pos2(0.0, y0), pos2(1.0, y0 + h))
    }
}

pub type Vec2Like = (f32, f32);

/// Paints an image cropped to fill `rect`. Returns false while it is still loading.
pub fn cover(ui: &Ui, uri: &str, rect: Rect, radius: f32, tint: Color32, focus: (f32, f32)) -> bool {
    let Some(tex) = texture(ui.ctx(), uri) else { return false };
    let uv = cover_uv((tex.size.x, tex.size.y), rect, focus);
    egui::Image::from_texture(tex)
        .uv(uv)
        .tint(tint)
        .corner_radius(CornerRadius::same(radius.round().clamp(0.0, 255.0) as u8))
        .paint_at(ui, rect);
    true
}

/// Paints an image scaled down to fit inside `rect`, anchored bottom-left. Returns the painted rect.
pub fn contain_left(ui: &Ui, uri: &str, rect: Rect, tint: Color32) -> Option<Rect> {
    let tex = texture(ui.ctx(), uri)?;
    let scale = (rect.width() / tex.size.x).min(rect.height() / tex.size.y);
    let size = tex.size * scale;
    let r = Rect::from_min_size(pos2(rect.left(), rect.bottom() - size.y), size);
    egui::Image::from_texture(tex).tint(tint).paint_at(ui, r);
    Some(r)
}

/// Vertical gradient from `top` to `bottom` colour.
pub fn vgradient(p: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(rect.left_top(), top);
    m.colored_vertex(rect.right_top(), top);
    m.colored_vertex(rect.left_bottom(), bottom);
    m.colored_vertex(rect.right_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(1, 3, 2);
    p.add(m);
}

/// Horizontal gradient from `left` to `right` colour.
pub fn hgradient(p: &Painter, rect: Rect, left: Color32, right: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(rect.left_top(), left);
    m.colored_vertex(rect.right_top(), right);
    m.colored_vertex(rect.left_bottom(), left);
    m.colored_vertex(rect.right_bottom(), right);
    m.add_triangle(0, 1, 2);
    m.add_triangle(1, 3, 2);
    p.add(m);
}

/// Soft glow around a rounded rect: a few widening, fading outlines.
pub fn glow(p: &Painter, rect: Rect, radius: f32, color: Color32, spread: f32, strength: f32) {
    let steps = 8;
    for i in 0..steps {
        let t = i as f32 / steps as f32;
        let grow = spread * t;
        let a = strength * (1.0 - t).powi(2) / steps as f32 * 2.0;
        p.rect_stroke(
            rect.expand(grow),
            CornerRadius::same((radius + grow).round().clamp(0.0, 255.0) as u8),
            egui::Stroke::new(spread / steps as f32 + 1.0, color.gamma_multiply(a)),
            egui::StrokeKind::Outside,
        );
    }
}

/// Stable colour for a game without art, derived from its name.
pub fn name_color(name: &str) -> Color32 {
    let mut h: u32 = 0;
    for c in name.chars() {
        h = h.wrapping_mul(31).wrapping_add(c as u32);
    }
    let hue = (h % 360) as f32 / 360.0;
    egui::ecolor::Hsva::new(hue, 0.55, 0.42, 1.0).into()
}

pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().filter(|w| w.chars().next().is_some_and(char::is_alphanumeric)).collect();
    match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect::<String>().to_uppercase(),
        [a, b, ..] => format!("{}{}", a.chars().next().unwrap(), b.chars().next().unwrap()).to_uppercase(),
    }
}

/// A button glyph like the controller's: ✕ ○ △ □ for PlayStation, A B Y X for Xbox.
pub fn pad_glyph(p: &Painter, center: Pos2, r: f32, label: &str, color: Color32) {
    p.circle_stroke(center, r, egui::Stroke::new(1.5_f32, Color32::from_white_alpha(170)));
    p.text(center + vec2(0.0, -r * 0.05), egui::Align2::CENTER_CENTER, label, bold(r * 1.15), color);
}

pub fn white(a: f32) -> Color32 {
    Color32::from_white_alpha((a.clamp(0.0, 1.0) * 255.0) as u8)
}

pub fn black(a: f32) -> Color32 {
    Color32::from_black_alpha((a.clamp(0.0, 1.0) * 255.0) as u8)
}
