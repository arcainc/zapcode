//! Demonstrate dead-slot accumulation: a churning agent builds large
//! temporaries between tool calls; the arena never frees, so every
//! snapshot carries all of them.

use zapcode_core::vm::VmState;
use zapcode_core::{ResourceLimits, Value, ZapcodeRun, ZapcodeSnapshot};

fn main() {
    let code = r#"
        async function main() {
            let last = 0;
            for (let i = 0; i < 8; i++) {
                // Big throwaway intermediate: dead after this iteration.
                const scratch = Array.from({ length: 500 }, (_, j) => ({ j, tag: "row" + j }));
                last = scratch.length;
                await callTool("step" + i);
            }
            return last;
        }
        main();
    "#;
    let runner = ZapcodeRun::new(
        code.to_string(),
        Vec::new(),
        vec!["callTool".to_string()],
        ResourceLimits::default(),
    )
    .unwrap();
    let mut state = runner.start(Vec::new()).unwrap();
    let mut hop = 0;
    loop {
        match state {
            VmState::Suspended { snapshot, .. } => {
                let bytes = snapshot.dump().unwrap();
                println!("hop {hop}: snapshot {} bytes", bytes.len());
                state = ZapcodeSnapshot::load(&bytes)
                    .unwrap()
                    .resume(Value::Int(1))
                    .unwrap()
                    .state;
                hop += 1;
            }
            VmState::Complete(v) => {
                println!("complete: {v:?}");
                break;
            }
            other => panic!("{other:?}"),
        }
    }
}
