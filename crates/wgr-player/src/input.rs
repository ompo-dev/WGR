//! Bloco de input da engine, transcrito de `public/play.html` do 5-web-port.
//!
//! Layout (`WasmInputBlock`): 256 bytes de teclas, dez palavras i32
//! (`mouseX`, `mouseY`, `dX`, `dY`, `dZ`, botões, foco, capturado, cabeça e
//! cauda do texto), 64 u16 de texto, quatro palavras de extensão
//! (`dbgDrawLimit`, `dbgFrameDraws`, `winW`, `winH`) e `startMode`.
//!
//! O host escreve; a engine lê direto da memória compartilhada. Toda escrita
//! usa as operações atômicas de `GuestMemory`, como no navegador.

use anyhow::{Result, ensure};
use wgr_runtime::shared::GuestMemory;

pub const KEY_BYTES: u64 = 256;
pub const WORD_COUNT: u64 = 10;
pub const TEXT_OFFSET: u64 = 296;
pub const TEXT_SLOTS: u64 = 64;
pub const EXT_OFFSET: u64 = 424;
pub const START_MODE_OFFSET: u64 = 440;
pub const BLOCK_BYTES: u64 = 444;

// Índices das palavras, iguais aos do play.html.
pub const MX: u64 = 0;
pub const MY: u64 = 1;
pub const DX: u64 = 2;
pub const DY: u64 = 3;
pub const DZ: u64 = 4;
pub const BTN: u64 = 5;
pub const FOCUS: u64 = 6;
pub const LOCKED: u64 = 7;
pub const HEAD: u64 = 8;
pub const TAIL: u64 = 9;

/// Modo inicial: 1 = história, 2 = sandbox (igual ao `play.html`).
pub const START_STORY: i32 = 1;
pub const START_SANDBOX: i32 = 2;

#[derive(Clone)]
pub struct InputBlock {
    memory: GuestMemory,
    base: u64,
}

impl InputBlock {
    pub fn new(memory: GuestMemory, base: u64) -> Result<Self> {
        ensure!(base.is_multiple_of(4), "Bloco de input desalinhado");
        ensure!(
            base.checked_add(BLOCK_BYTES)
                .is_some_and(|end| end <= memory.len() as u64),
            "Bloco de input fora da memória"
        );
        Ok(Self { memory, base })
    }

    fn word_address(&self, index: u64) -> u64 {
        self.base + KEY_BYTES + index * 4
    }

    fn ext_address(&self, index: u64) -> u64 {
        self.base + EXT_OFFSET + index * 4
    }

    pub fn load_word(&self, index: u64) -> Result<i32> {
        Ok(self.memory.load_u32(self.word_address(index))? as i32)
    }

    pub fn store_word(&self, index: u64, value: i32) -> Result<()> {
        self.memory
            .store_u32(self.word_address(index), value as u32)
    }

    pub fn add_word(&self, index: u64, value: i32) -> Result<()> {
        self.memory.add_u32(self.word_address(index), value as u32)
    }

    pub fn size(&self) -> Result<(i32, i32)> {
        let width = self.memory.load_u32(self.ext_address(2))? as i32;
        let height = self.memory.load_u32(self.ext_address(3))? as i32;
        Ok((width.max(1), height.max(1)))
    }

    pub fn set_size(&self, width: i32, height: i32) -> Result<()> {
        self.memory.store_u32(self.ext_address(2), width as u32)?;
        self.memory.store_u32(self.ext_address(3), height as u32)
    }

    pub fn key(&self, virtual_key: u8) -> Result<u8> {
        Ok(self.memory.read(self.base + u64::from(virtual_key), 1)?[0])
    }

    pub fn set_key(&self, virtual_key: u8, down: bool) -> Result<()> {
        self.memory
            .write(self.base + u64::from(virtual_key), &[u8::from(down) << 7])
    }

    pub fn set_buttons(&self, bits: i32) -> Result<()> {
        self.store_word(BTN, bits)
    }

    pub fn set_focus(&self, focused: bool) -> Result<()> {
        self.store_word(FOCUS, i32::from(focused))
    }

    pub fn set_locked(&self, locked: bool) -> Result<()> {
        self.store_word(LOCKED, i32::from(locked))
    }

    pub fn set_start_mode(&self, mode: i32) -> Result<()> {
        self.memory
            .store_u32(self.base + START_MODE_OFFSET, mode as u32)
    }

    pub fn start_mode(&self) -> Result<i32> {
        Ok(self.memory.load_u32(self.base + START_MODE_OFFSET)? as i32)
    }

    /// Texto de um caractere (o mesmo comportamento do `keydown` do play.html).
    pub fn push_text(&self, utf16: u16) -> Result<bool> {
        let head = self.load_word(HEAD)?;
        let tail = self.load_word(TAIL)?;
        if head.wrapping_sub(tail) as u32 >= TEXT_SLOTS as u32 {
            return Ok(false);
        }
        let slot = self.base + TEXT_OFFSET + u64::from(head as u32 % TEXT_SLOTS as u32) * 2;
        self.memory.write(slot, &utf16.to_le_bytes())?;
        self.store_word(HEAD, head.wrapping_add(1))?;
        Ok(true)
    }

    /// Movimento relativo com o ponteiro capturado: igual ao ramo `locked()`
    /// do `mousemove`. Acumula em `dX/dY` e move a posição absoluta com clamp.
    pub fn push_relative(&self, dx: i32, dy: i32) -> Result<()> {
        self.add_word(DX, dx)?;
        self.add_word(DY, dy)?;
        let (width, height) = self.size()?;
        let x = (self.load_word(MX)?.saturating_add(dx)).clamp(0, width - 1);
        let y = (self.load_word(MY)?.saturating_add(dy)).clamp(0, height - 1);
        self.store_word(MX, x)?;
        self.store_word(MY, y)
    }

    /// Movimento absoluto (ponteiro livre): converte para delta e posição,
    /// como o ramo não capturado do `mousemove`.
    pub fn set_absolute(&self, x: i32, y: i32) -> Result<()> {
        let (width, height) = self.size()?;
        let x = x.clamp(0, width - 1);
        let y = y.clamp(0, height - 1);
        self.add_word(DX, x - self.load_word(MX)?)?;
        self.add_word(DY, y - self.load_word(MY)?)?;
        self.store_word(MX, x)?;
        self.store_word(MY, y)
    }

    /// Centraliza a posição absoluta ao publicar o bloco.
    pub fn center(&self) -> Result<()> {
        let (width, height) = self.size()?;
        self.store_word(MX, width / 2)?;
        self.store_word(MY, height / 2)
    }
}

/// Encaixa o buffer da engine na janela mantendo a proporção, como o blit do
/// 5-web-port. Retorna (x, y, largura, altura) em pixels da janela.
pub fn content_viewport(
    width: u32,
    height: u32,
    render_width: u32,
    render_height: u32,
) -> (f32, f32, f32, f32) {
    if width == 0 || height == 0 || render_width == 0 || render_height == 0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let scale = (width as f32 / render_width as f32).min(height as f32 / render_height as f32);
    let w = (render_width as f32 * scale).min(width as f32);
    let h = (render_height as f32 * scale).min(height as f32);
    ((width as f32 - w) * 0.5, (height as f32 - h) * 0.5, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgr_runtime::NativeRuntime;

    fn block() -> InputBlock {
        let runtime = NativeRuntime::new().unwrap();
        let memory = GuestMemory::new64(runtime.engine(), 1, 1, 65536).unwrap();
        let block = InputBlock::new(memory, 0).unwrap();
        block.set_size(1920, 1200).unwrap();
        block.center().unwrap();
        block
    }

    #[test]
    fn uses_the_layout_from_the_web_page() {
        let block = block();
        assert_eq!(block.load_word(MX).unwrap(), 960);
        assert_eq!(block.load_word(MY).unwrap(), 600);
        block.set_key(0x57, true).unwrap();
        assert_eq!(block.key(0x57).unwrap(), 0x80);
        block.set_key(0x57, false).unwrap();
        assert_eq!(block.key(0x57).unwrap(), 0);
    }

    #[test]
    fn relative_movement_accumulates_and_clamps_the_absolute_position() {
        let block = block();
        block.push_relative(30, -40).unwrap();
        assert_eq!(block.load_word(DX).unwrap(), 30);
        assert_eq!(block.load_word(DY).unwrap(), -40);
        assert_eq!(block.load_word(MX).unwrap(), 990);
        assert_eq!(block.load_word(MY).unwrap(), 560);
        block.store_word(MX, 1919).unwrap();
        block.store_word(MY, 0).unwrap();
        block.push_relative(50, -50).unwrap();
        assert_eq!(block.load_word(DX).unwrap(), 80);
        assert_eq!(block.load_word(MX).unwrap(), 1919);
        assert_eq!(block.load_word(MY).unwrap(), 0);
    }

    #[test]
    fn absolute_movement_converts_the_delta_like_the_unlocked_branch() {
        let block = block();
        block.set_absolute(1000, 700).unwrap();
        assert_eq!(block.load_word(DX).unwrap(), 40);
        assert_eq!(block.load_word(DY).unwrap(), 100);
        assert_eq!(block.load_word(MX).unwrap(), 1000);
        assert_eq!(block.load_word(MY).unwrap(), 700);
    }

    #[test]
    fn viewport_fits_letterboxed_windows() {
        assert_eq!(
            content_viewport(1920, 1200, 1280, 720),
            (0.0, 60.0, 1920.0, 1080.0)
        );
        assert_eq!(content_viewport(0, 0, 1280, 720), (0.0, 0.0, 0.0, 0.0));
        assert_eq!(
            content_viewport(1280, 720, 1280, 720),
            (0.0, 0.0, 1280.0, 720.0)
        );
    }
}
