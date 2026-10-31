//! Fixture wat do contrato de pthread/invoke: cria a thread via
//! `__pthread_create_js`, executa a rotina na tabela e chama `invoke_ij`.

use std::time::{Duration, Instant};
use wgr_player::engine;

const FIXTURE: &str = r#"
(module
  (import "env" "memory" (memory i64 1 2 shared))
  (import "env" "__pthread_create_js" (func $pthread (param i64 i64 i64 i64) (result i32)))
  (import "env" "invoke_ij" (func $invoke (param i64 i64) (result i32)))
  (import "env" "wgpu_start_worker" (func $gpu (param i64 i64 i64)))
  (type $thread_t (func (param i64)))
  (type $invoke_t (func (param i64) (result i32)))
  (table 4 funcref)
  (export "__indirect_function_table" (table 0))
  (elem (i32.const 0) $thread_start $invoked)
  (func $thread_start (type $thread_t) (param $arg i64)
    (i64.store (i64.const 128) (i64.const 99)))
  (func $invoked (type $invoke_t) (param $arg i64) (result i32)
    i32.const 5)
  (func (export "_emscripten_stack_alloc") (param i64) (result i64) i64.const 4096)
  (func (export "_emscripten_thread_init") (param i64 i32 i32 i32 i64 i32))
  (func (export "emscripten_stack_set_limits") (param i64 i64))
  (func (export "_emscripten_stack_restore") (param i64))
  (func (export "__main_argc_argv") (param i32 i64) (result i32)
    (drop (call $pthread (i64.const 0) (i64.const 0) (i64.const 0) (i64.const 0)))
    (drop (call $invoke (i64.const 1) (i64.const 9)))
    (call $gpu (i64.const 0) (i64.const 4096) (i64.const 0))
    i32.const 0)
)
"#;

#[test]
fn pthread_and_invoke_contract_runs_the_table_routine() {
    let bytes = wat::parse_str(FIXTURE).expect("wat");
    let mut host = engine::load(&bytes, 2, 4 << 20).expect("host");
    let code = host.start(&["fixture".into()]).expect("main");
    assert_eq!(code, 0);
    assert!(host.gpu_started(), "bootstrap da GPU não chamado");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let marker = host
            .memory
            .read(128, 8)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .map(u64::from_le_bytes);
        if marker == Some(99) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a thread não executou a rotina da tabela: {marker:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
