//! Regressões offline da ordem TLS, thread principal e construtores.

use wgr_player::engine;

const FIXTURE: &str = r#"
(module
  (import "env" "memory" (memory i64 1 2 shared))
  (import "env" "_emscripten_init_main_thread_js" (func $init_main (param i64)))
  (global $tls (mut i64) (i64.const 0))
  (func (export "_emscripten_tls_init") (result i64)
    (global.set $tls (i64.const 128))
    (global.get $tls))
  (func (export "_emscripten_thread_init") (param i64 i32 i32 i32 i32 i32)
    (i32.store (i64.const 260) (local.get 2))
    (i32.store (i64.const 264) (local.get 3)))
  (func (export "__wasm_call_ctors")
    (if (i64.ne (global.get $tls) (i64.const 128)) (then unreachable))
    (call $init_main (i64.const 512))
    (if (i32.ne (i32.load (i64.const 260)) (i32.const 1)) (then unreachable))
    (if (i32.ne (i32.load (i64.const 264)) (i32.const 1)) (then unreachable))
    (i32.store (i64.const 268) (i32.const 1)))
  (global $stack (mut i64) (i64.const 8192))
  (func (export "_emscripten_stack_alloc") (param $size i64) (result i64)
    (global.set $stack (i64.sub (global.get $stack) (local.get $size)))
    (global.get $stack))
  (func (export "__main_argc_argv") (param i32 i64) (result i32)
    (if (i32.ne (i32.load (i64.const 268)) (i32.const 1)) (then unreachable))
    i32.const 0)
)
"#;

#[test]
fn initializes_tls_and_main_thread_before_the_constructors() {
    let bytes = wat::parse_str(FIXTURE).unwrap();
    let mut host = engine::load(&bytes, 2, 4 << 20).unwrap();
    assert_eq!(host.start(&["fixture".into()]).unwrap(), 0);
    assert_eq!(host.memory.load_u32(268).unwrap(), 1);
}

#[test]
fn reports_an_invalid_tls_signature_instead_of_skipping_initialization() {
    let fixture = FIXTURE.replace(
        "(export \"_emscripten_tls_init\") (result i64)",
        "(export \"_emscripten_tls_init\") (param i32) (result i64)",
    );
    let bytes = wat::parse_str(fixture).unwrap();
    let mut host = engine::load(&bytes, 2, 4 << 20).unwrap();
    let error = host.start(&[]).unwrap_err().to_string();
    assert!(error.contains("_emscripten_tls_init"), "{error}");
}

#[test]
fn heap_max_uses_the_budgeted_limit_and_module_options_keep_their_default() {
    let bytes = wat::parse_str(
        r#"(module
      (import "env" "memory" (memory i64 1 4 shared))
      (import "env" "emscripten_get_heap_max" (func $maximum (result i64)))
      (import "env" "wasm_module_int_js" (func $option (param i64 i32) (result i32)))
      (func (export "maximum") (result i64) call $maximum)
      (func (export "option") (result i32) i64.const 128 i32.const 60 call $option))"#,
    )
    .unwrap();
    let mut host = engine::load(&bytes, 2, 131072).unwrap();
    assert_eq!(host.memory.len(), 65536);
    let maximum = host
        .instance
        .get_typed_func::<(), i64>(&mut host.store, "maximum")
        .unwrap();
    assert_eq!(maximum.call(&mut host.store, ()).unwrap(), 131072);
    let option = host
        .instance
        .get_typed_func::<(), i32>(&mut host.store, "option")
        .unwrap();
    assert_eq!(option.call(&mut host.store, ()).unwrap(), 60);
}
