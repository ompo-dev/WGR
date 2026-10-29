use gpui::{App, Font, Pixels, px};
use theme::{ThemeSettingsProvider, UiDensity};
#[derive(rust_embed::RustEmbed)]
#[folder = "../../assets/momor/"]
pub struct Assets;
impl gpui::AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        Ok(Self::get(path).map(|asset| asset.data))
    }
    fn list(&self, path: &str) -> anyhow::Result<Vec<gpui::SharedString>> {
        Ok(Self::iter()
            .filter(|name| name.starts_with(path))
            .map(|name| gpui::SharedString::from(name.into_owned()))
            .collect())
    }
}
pub fn init(cx: &mut App) {
    theme::init(theme::LoadThemes::JustBase, cx);
    theme::set_theme_settings_provider(
        Box::new(Fonts {
            ui: gpui::font("Segoe UI"),
            buffer: gpui::font("Consolas"),
        }),
        cx,
    );
    momor_menu::init();
    cx.bind_keys([
        gpui::KeyBinding::new("up", momor_menu::SelectPrevious, Some("menu")),
        gpui::KeyBinding::new("down", momor_menu::SelectNext, Some("menu")),
        gpui::KeyBinding::new("enter", momor_menu::Confirm, Some("menu")),
        gpui::KeyBinding::new("escape", momor_menu::Cancel, Some("menu")),
        gpui::KeyBinding::new("home", momor_menu::SelectFirst, Some("menu")),
        gpui::KeyBinding::new("end", momor_menu::SelectLast, Some("menu")),
    ]);
}
struct Fonts {
    ui: Font,
    buffer: Font,
}
impl ThemeSettingsProvider for Fonts {
    fn ui_font<'a>(&'a self, _: &'a App) -> &'a Font {
        &self.ui
    }
    fn buffer_font<'a>(&'a self, _: &'a App) -> &'a Font {
        &self.buffer
    }
    fn ui_font_size(&self, _: &App) -> Pixels {
        px(14.)
    }
    fn buffer_font_size(&self, _: &App) -> Pixels {
        px(13.)
    }
    fn ui_density(&self, _: &App) -> UiDensity {
        UiDensity::Default
    }
}
