pub(crate) struct Scratch(pub std::path::PathBuf);

#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct RenderedText(std::sync::Mutex<std::collections::BTreeSet<String>>);
#[cfg(debug_assertions)]
impl RenderedText {
    pub(crate) fn lines(&self) -> std::collections::BTreeSet<String> {
        self.0.lock().unwrap().clone()
    }
}
#[cfg(debug_assertions)]
impl gpui::PlatformTextSystem for RenderedText {
    fn add_fonts(&self, fonts: Vec<std::borrow::Cow<'static, [u8]>>) -> anyhow::Result<()> {
        gpui::NoopTextSystem.add_fonts(fonts)
    }
    fn all_font_names(&self) -> Vec<String> {
        gpui::NoopTextSystem.all_font_names()
    }
    fn font_id(&self, font: &gpui::Font) -> anyhow::Result<gpui::FontId> {
        gpui::NoopTextSystem.font_id(font)
    }
    fn font_metrics(&self, font: gpui::FontId) -> gpui::FontMetrics {
        gpui::NoopTextSystem.font_metrics(font)
    }
    fn typographic_bounds(
        &self,
        font: gpui::FontId,
        glyph: gpui::GlyphId,
    ) -> anyhow::Result<gpui::Bounds<f32>> {
        gpui::NoopTextSystem.typographic_bounds(font, glyph)
    }
    fn advance(&self, font: gpui::FontId, glyph: gpui::GlyphId) -> anyhow::Result<gpui::Size<f32>> {
        gpui::NoopTextSystem.advance(font, glyph)
    }
    fn glyph_for_char(&self, font: gpui::FontId, ch: char) -> Option<gpui::GlyphId> {
        gpui::NoopTextSystem.glyph_for_char(font, ch)
    }
    fn glyph_raster_bounds(
        &self,
        params: &gpui::RenderGlyphParams,
    ) -> anyhow::Result<gpui::Bounds<gpui::DevicePixels>> {
        gpui::NoopTextSystem.glyph_raster_bounds(params)
    }
    fn rasterize_glyph(
        &self,
        params: &gpui::RenderGlyphParams,
        bounds: gpui::Bounds<gpui::DevicePixels>,
    ) -> anyhow::Result<(gpui::Size<gpui::DevicePixels>, Vec<u8>)> {
        gpui::NoopTextSystem.rasterize_glyph(params, bounds)
    }
    fn layout_line(
        &self,
        text: &str,
        size: gpui::Pixels,
        runs: &[gpui::FontRun],
    ) -> gpui::LineLayout {
        self.0.lock().unwrap().insert(text.to_owned());
        gpui::NoopTextSystem.layout_line(text, size, runs)
    }
    fn recommended_rendering_mode(
        &self,
        font: gpui::FontId,
        size: gpui::Pixels,
    ) -> gpui::TextRenderingMode {
        gpui::NoopTextSystem.recommended_rendering_mode(font, size)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.0) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("remove owned scratch: {error}"),
        }
    }
}

#[test]
fn scratch_removes_setup_failures_and_absent_folders() {
    let root = std::path::Path::new("/tmp").join(format!(
        "veyra-scratch-{}",
        veyra_engine::domain::ThreadId::new()
    ));
    assert!(
        std::panic::catch_unwind(|| {
            let _scratch = Scratch(root.clone());
            std::fs::create_dir(&root).unwrap();
            panic!("setup failed");
        })
        .is_err()
    );
    assert!(!root.exists());
    drop(Scratch(root.clone()));
    assert!(!root.exists());
}
