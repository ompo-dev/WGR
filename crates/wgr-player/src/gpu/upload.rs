//! Leituras de upload do ring (`UPLOAD_BUFFER`/`UPLOAD_BUFFER_NO_OVERWRITE` e
//! `UPLOAD_TEXTURE`), com a mesma geometria usada pelo `wgpu_worker.js`.
//! O endereço dos bytes é absoluto na memória da engine.

use wgpu::TextureFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferUpload {
    pub id: u32,
    pub offset: u64,
    pub size: u64,
    pub data_address: u64,
    pub no_overwrite: bool,
}

/// `UPLOAD_BUFFER` (12) e `UPLOAD_BUFFER_NO_OVERWRITE` (47): id, offset, size e
/// o endereço dos dados logo após o cabeçalho.
pub fn parse_buffer_upload(
    payload_address: u64,
    words: &[u32],
    no_overwrite: bool,
) -> Option<BufferUpload> {
    let get = |index: usize| words.get(index).copied();
    Some(BufferUpload {
        id: get(0)?,
        offset: u64::from(get(1)?),
        size: u64::from(get(2)?),
        data_address: payload_address.checked_add(12)?,
        no_overwrite,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureUpload {
    pub id: u32,
    pub mip: u32,
    pub slice: u32,
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub bytes_per_row: u32,
    pub rows: u32,
    pub size: u64,
    pub data_address: u64,
}

/// `UPLOAD_TEXTURE` (13): id, mip, slice, x, y, z, largura, altura,
/// profundidade, bytes por linha, linhas e tamanho; dados em seguida.
pub fn parse_texture_upload(payload_address: u64, words: &[u32]) -> Option<TextureUpload> {
    let get = |index: usize| words.get(index).copied();
    Some(TextureUpload {
        id: get(0)?,
        mip: get(1)?,
        slice: get(2)?,
        x: get(3)?,
        y: get(4)?,
        z: get(5)?,
        width: get(6)?,
        height: get(7)?,
        depth: get(8)?,
        bytes_per_row: get(9)?,
        rows: get(10)?,
        size: u64::from(get(11)?),
        data_address: payload_address.checked_add(12 * 4)?,
    })
}

/// Confere a geometria de um upload de textura contra o formato e o tamanho.
/// Devolve `false` quando a cópia sairia da textura ou da memória.
pub fn valid_texture_upload(upload: &TextureUpload, format: TextureFormat) -> bool {
    let (block_w, block_h) = format.block_dimensions();
    let block_bytes = format.block_copy_size(None).unwrap_or(4);
    let (Some(row_bytes), Some(min_rows)) = (
        upload.width.div_ceil(block_w).checked_mul(block_bytes),
        Some(upload.height.div_ceil(block_h)),
    ) else {
        return false;
    };
    if upload.width == 0 || upload.height == 0 || upload.depth == 0 {
        return false;
    }
    if upload.bytes_per_row < row_bytes || upload.rows < min_rows {
        return false;
    }
    let Some(required) = (upload.bytes_per_row as u64)
        .checked_mul(u64::from(upload.rows))
        .and_then(|value| value.checked_mul(u64::from(upload.depth)))
    else {
        return false;
    };
    required <= upload.size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_buffer_uploads_with_an_absolute_data_address() {
        let upload = parse_buffer_upload(1024, &[5, 0, 256], false).unwrap();
        assert_eq!(upload.id, 5);
        assert_eq!(upload.size, 256);
        assert_eq!(upload.data_address, 1024 + 12);
        assert!(!upload.no_overwrite);
        let copy = parse_buffer_upload(1024, &[5, 16, 32], true).unwrap();
        assert!(copy.no_overwrite);
    }

    #[test]
    fn parses_texture_uploads_with_all_geometry() {
        let upload =
            parse_texture_upload(2048, &[8, 0, 1, 4, 8, 0, 64, 32, 1, 256, 32, 8192]).unwrap();
        assert_eq!(upload.id, 8);
        assert_eq!(upload.slice, 1);
        assert_eq!((upload.x, upload.y, upload.z), (4, 8, 0));
        assert_eq!((upload.width, upload.height, upload.depth), (64, 32, 1));
        assert_eq!(upload.bytes_per_row, 256);
        assert_eq!(upload.rows, 32);
        assert_eq!(upload.data_address, 2048 + 48);
    }

    #[test]
    fn validates_rgba8_and_block_compressed_rows() {
        let rgba = TextureUpload {
            id: 1,
            mip: 0,
            slice: 0,
            x: 0,
            y: 0,
            z: 0,
            width: 64,
            height: 32,
            depth: 1,
            bytes_per_row: 256,
            rows: 32,
            size: 8192,
            data_address: 0,
        };
        assert!(valid_texture_upload(&rgba, TextureFormat::Rgba8Unorm));
        let mut narrow = rgba;
        narrow.bytes_per_row = 128;
        assert!(!valid_texture_upload(&narrow, TextureFormat::Rgba8Unorm));
        let bc1 = TextureUpload {
            width: 64,
            height: 32,
            bytes_per_row: 128,
            rows: 8,
            size: 1024,
            ..rgba
        };
        assert!(valid_texture_upload(&bc1, TextureFormat::Bc1RgbaUnorm));
    }
}
