//! Uniformes reais nos dois estágios, com cópias independentes por desenho.

mod common;

use common::read_pixel;
use wgpu::{DeviceDescriptor, Instance, RequestAdapterOptions};
use wgr_player::gpu::shaders::{ShaderIndex, ShaderSource};
use wgr_player::gpu::worker::{Object, Worker};
use wgr_runtime::{NativeRuntime, shared::GuestMemory};

const VS: &str = r#"
struct Color { values : array<vec4<f32>, 1>, }
@group(0) @binding(3) var<uniform> vertex_color : Color;
struct VOut { @builtin(position) pos : vec4<f32>, @location(0) color : vec4<f32>, }
@vertex fn main(@builtin(vertex_index) i : u32) -> VOut {
  var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(-1.0, 3.0), vec2<f32>(3.0, -1.0));
  return VOut(vec4<f32>(p[i], 0.0, 1.0), vertex_color.values[0]);
}
"#;

// Declaração fora da ordem de binding: o pack precisa ordenar os membros.
const PS: &str = r#"
struct Pair { values : array<vec4<f32>, 2>, }
struct Color { values : array<vec4<f32>, 1>, }
@group(1) @binding(5) var<uniform> alpha : Color;
@group(1) @binding(2) var<uniform> fragment_color : Pair;
@fragment fn main(@location(0) vertex_color : vec4<f32>) -> @location(0) vec4<f32> {
  return vec4<f32>(vertex_color.rgb + fragment_color.values[0].rgb + fragment_color.values[1].rgb, alpha.values[0].x);
}
"#;

struct Fixture {
    worker: Worker,
    memory: GuestMemory,
}

impl Fixture {
    fn new(vs: &str, ps: &str) -> Self {
        let adapter = pollster::block_on(
            Instance::default().request_adapter(&RequestAdapterOptions::default()),
        )
        .expect("fixture requer adaptador GPU; ausência não conta como sucesso");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&DeviceDescriptor::default())).unwrap();
        let runtime = NativeRuntime::new().unwrap();
        let memory = GuestMemory::new64(runtime.engine(), 1, 1, 1 << 20).unwrap();
        let mut worker = Worker::new(device, queue);
        worker.register_shader_source("0000000000000001", vs);
        worker.register_shader_source("0000000000000002", ps);
        let mut fixture = Self { worker, memory };
        fixture.exec(14, &[10, 0, 1, 0, 0]);
        fixture.exec(14, &[11, 1, 2, 0, 0]);
        fixture.exec(18, &[0, 10]);
        fixture.exec(18, &[1, 11]);
        fixture.exec(20, &[4]);
        fixture
    }

    fn exec(&mut self, op: u32, words: &[u32]) {
        self.worker.exec(op, words, 0, &self.memory).unwrap();
    }

    fn cb(&mut self, id: u32, values: &[[f32; 4]]) {
        let bytes: Vec<_> = values
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        if self.worker.objects.get(id).is_none() {
            self.exec(11, &[id, bytes.len() as u32, 4, 0, 0]);
        }
        self.memory.write(1036, &bytes).unwrap();
        self.worker
            .exec(12, &[id, 0, bytes.len() as u32], 1024, &self.memory)
            .unwrap();
    }

    fn draw(&mut self, target: u32) {
        self.exec(3, &[target, 2, 4, 4, 1, 1, 1, 28, 0x20, 1]);
        let mut targets = [0; 37];
        targets[0] = target;
        self.exec(26, &targets);
        self.exec(32, &[0, 3, 1, 0, 0, 0]);
    }

    fn pixel(&self, target: u32) -> [u8; 4] {
        read_pixel(
            &self.worker.device,
            &self.worker.queue,
            &self.worker,
            target,
        )
    }
}

#[test]
fn packs_both_stages_offsets_missing_tails_and_snapshots() {
    let mut f = Fixture::new(VS, PS);
    // first=1 pula o registro sentinela; size=0 não limita packCopy no JS.
    f.cb(20, &[[9.0; 4], [1.0, 0.0, 0.0, 0.0]]);
    f.cb(21, &[[9.0; 4], [0.0, 1.0, 0.0, 0.0]]);
    f.cb(22, &[[1.0, 0.0, 0.0, 0.0]]);
    f.exec(23, &[0, 3, 1, 20, 1, 0]);
    f.exec(23, &[1, 2, 1, 21, 1, 0]);
    f.exec(23, &[1, 5, 1, 22, 0, 1]);
    f.draw(30);
    // Mesmo recurso/first, conteúdo novo; não pode alterar o primeiro draw.
    f.cb(20, &[[9.0; 4], [0.0, 0.0, 1.0, 0.0]]);
    f.cb(21, &[[9.0; 4], [0.0; 4]]);
    assert_eq!(
        f.pixel(30),
        [0; 4],
        "upload somente CPU não submete o desenho pendente"
    );
    f.draw(31);
    // Alteração direta do shadow também precisa ser isolada no draw seguinte.
    if let Some(Object::Buffer(buffer)) = f.worker.objects.get_mut(20) {
        buffer.shadow.as_mut().unwrap()[16..32].fill(0);
    }
    f.exec(23, &[1, 2, 1, 999, 0, 0]);
    f.exec(23, &[1, 5, 1, 0, 0, 0]);
    f.draw(32);
    // first além do EOF deve zerar todo o membro, não ler fora do shadow.
    f.exec(23, &[0, 3, 1, 20, u32::MAX, 1]);
    f.exec(23, &[1, 2, 1, 21, u32::MAX, 1]);
    f.exec(23, &[1, 5, 1, 22, 0, 1]);
    f.draw(33);
    f.worker.submit();
    assert_eq!(f.pixel(30), [255, 255, 0, 255]);
    assert_eq!(f.pixel(31), [0, 0, 255, 255]);
    assert_eq!(f.pixel(32), [0, 0, 0, 0]);
    assert_eq!(f.pixel(33), [0, 0, 0, 255]);
}

#[test]
fn inline_tables_from_a_pack_work_alongside_dynamic_uniforms_in_both_stages() {
    let vs = format!("{}{}", r#"
struct Inline { values : array<vec4<u32>, 2>, }
@group(0) @binding(15) var<uniform> inline_table : Inline;
"#, VS.replace("vertex_color.values[0]", "vertex_color.values[0] + vec4<f32>(bitcast<f32>(inline_table.values[0].x), 0.0, 0.0, 0.0)"));
    let ps = format!(
        "{}{}",
        r#"
struct Inline { values : array<vec4<u32>, 2>, }
@group(1) @binding(15) var<uniform> inline_table : Inline;
"#,
        PS.replace(
            "alpha.values[0].x);",
            "alpha.values[0].x) + vec4<f32>(0.0, 0.0, bitcast<f32>(inline_table.values[1].x), 0.0);"
        )
    );
    let mut f = Fixture::new(&vs, &ps);
    // Mesmo formato concatenado usado por source_from_pack; sem rede.
    let constants = "[0,1065353216]";
    let pack = format!("{ps}{constants}");
    let index = ShaderIndex::parse(
        &serde_json::json!({
            "0000000000000002": {
                "stage": "ps", "effect": "fixture", "program": "inline", "ok": true,
                "p": [0, 0, ps.len(), constants.len()], "consts": true
            }
        })
        .to_string(),
    )
    .unwrap();
    f.worker.register_shader_with_constants(
        "0000000000000002",
        index
            .source_from_pack("0000000000000002", pack.as_bytes())
            .unwrap()
            .unwrap(),
    );
    f.worker.register_shader_with_constants(
        "0000000000000001",
        ShaderSource {
            text: vs,
            constants: Some("[1065353216,0]".into()),
        },
    );
    f.exec(14, &[10, 0, 1, 0, 0]);
    f.exec(14, &[11, 1, 2, 0, 0]);
    f.cb(22, &[[1.0, 0.0, 0.0, 0.0]]);
    f.exec(23, &[1, 5, 1, 22, 0, 1]);
    f.draw(30);
    f.worker.submit();
    assert_eq!(
        f.pixel(30),
        [255, 0, 255, 255],
        "raw words e padding de 16 bytes nos dois estágios"
    );
}

#[test]
fn empty_inline_table_is_zeroed_and_missing_table_fails_explicitly() {
    let ps = r#"
struct Inline { values : array<vec4<u32>, 1>, }
@group(1) @binding(15) var<uniform> inline_table : Inline;
@fragment fn main() -> @location(0) vec4<f32> {
  return vec4<f32>(f32(inline_table.values[0].x), 1.0, 0.0, 1.0);
}
"#;
    let mut f = Fixture::new(VS, ps);
    f.exec(3, &[30, 2, 4, 4, 1, 1, 1, 28, 0x20, 1]);
    let mut targets = [0; 37];
    targets[0] = 30;
    f.exec(26, &targets);
    let error = f
        .worker
        .exec(32, &[0, 3, 1, 0, 0, 0], 0, &f.memory)
        .unwrap_err();
    assert!(error.to_string().contains("Binding 15 sem tabela"));
    f.worker.register_shader_with_constants(
        "0000000000000002",
        ShaderSource {
            text: ps.into(),
            constants: Some("[]".into()),
        },
    );
    f.exec(14, &[11, 1, 2, 0, 0]);
    f.exec(32, &[0, 3, 1, 0, 0, 0]);
    f.worker.submit();
    assert_eq!(f.pixel(30), [0, 255, 0, 255]);
}

#[test]
fn rejects_excessive_packs_and_unsupported_storage_without_partial_bind_groups() {
    let mut f = Fixture::new(VS, PS);
    for count in [4097u64, u64::MAX / 16, u64::MAX] {
        let text = PS.replace("array<vec4<f32>, 2>", &format!("array<vec4<f32>, {count}>"));
        f.worker.register_shader_source("0000000000000003", &text);
        assert!(f.worker.exec(14, &[12, 1, 3, 0, 0], 0, &f.memory).is_err());
        assert!(
            f.worker.objects.get(12).is_none(),
            "não publica shader inválido"
        );
    }
    let storage = r#"
@group(1) @binding(32) var<storage, read> values : array<vec4<f32>>;
@fragment fn main() -> @location(0) vec4<f32> { return values[0]; }
"#;
    f.worker.register_shader_source("0000000000000003", storage);
    f.exec(14, &[12, 1, 3, 0, 0]);
    f.exec(18, &[1, 12]);
    f.exec(3, &[30, 2, 4, 4, 1, 1, 1, 28, 0x20, 1]);
    let mut targets = [0; 37];
    targets[0] = 30;
    f.exec(26, &targets);
    let error = f
        .worker
        .exec(32, &[0, 3, 1, 0, 0, 0], 0, &f.memory)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Tipo de binding ainda não suportado")
    );
    f.worker.submit();
}

#[test]
fn caps_pending_snapshot_memory_and_resumes_the_same_render_target() {
    // PS usa exatamente 64 KiB; com 16 bytes do VS, o draw 256 precisa submeter.
    let ps = PS.replace("array<vec4<f32>, 2>", "array<vec4<f32>, 4095>");
    let mut f = Fixture::new(VS, &ps);
    f.cb(20, &[[1.0, 0.0, 0.0, 0.0]]);
    f.cb(22, &[[1.0, 0.0, 0.0, 0.0]]);
    f.exec(23, &[0, 3, 1, 20, 0, 1]);
    f.exec(23, &[1, 5, 1, 22, 0, 1]);
    f.draw(30);
    assert_eq!(f.pixel(30), [0; 4]);
    f.cb(20, &[[0.0, 0.0, 1.0, 0.0]]);
    for _ in 1..256 {
        f.exec(32, &[0, 3, 1, 0, 0, 0]);
    }
    assert_eq!(
        f.pixel(30),
        [0, 0, 255, 255],
        "o limite submeteu os desenhos anteriores"
    );
    f.cb(20, &[[0.0, 1.0, 0.0, 0.0]]);
    f.exec(32, &[0, 3, 1, 0, 0, 0]);
    f.worker.submit();
    assert_eq!(f.pixel(30), [0, 255, 0, 255]);
}
