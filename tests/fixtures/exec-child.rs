use std::{io::{self, Read, Write}, process::ExitCode};

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args[0].as_str() {
        "echo" | "test" => {
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input).unwrap();
            io::stdout().write_all(&input).unwrap();
            println!("{:?}", &args[1..]);
            println!("{}", std::env::var("EXEC_VALUE").unwrap());
            println!("{}", std::env::current_dir().unwrap().display());
            eprintln!("separate stderr");
            ExitCode::from(37)
        }
        "read" => {
            io::stdout().write_all(&std::fs::read(&args[1]).unwrap()).unwrap();
            ExitCode::SUCCESS
        }
        "json" | "build" => {
            std::fs::OpenOptions::new().create(true).append(true).open("executed").unwrap().write_all(b"once").unwrap();
            let value = format!("{{\n  \"items\": [\n{}\n  ]\n}}\n", (0..100).map(|n| format!("    {{ \"id\": {n}, \"name\": \"item\" }}")).collect::<Vec<_>>().join(",\n"));
            print!("{value}");
            eprint!("{}", value.replace("\"item\"", "\"error\""));
            ExitCode::from(19)
        }
        #[cfg(windows)]
        "cancel-wrapper" => {
            use std::os::windows::process::CommandExt;
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn AllocConsole() -> i32;
                fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
            }
            unsafe { AllocConsole(); }
            let mut command = std::process::Command::new(&args[1]);
            command.arg("exec");
            if args[2] == "direct" { command.arg("--no-compress"); }
            let mut child = command.arg("--").arg(std::env::current_exe().unwrap()).arg("tree")
                .creation_flags(0x200).spawn().unwrap(); // CREATE_NEW_PROCESS_GROUP
            let start = std::time::Instant::now();
            while !std::path::Path::new("ready").exists() {
                assert!(start.elapsed() < std::time::Duration::from_secs(5));
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_ne!(unsafe { GenerateConsoleCtrlEvent(1, child.id()) }, 0); // CTRL_BREAK_EVENT
            assert!(!child.wait().unwrap().success());
            ExitCode::SUCCESS
        }
        "large" => {
            let out = std::thread::spawn(|| io::stdout().write_all(&vec![b'o'; 9 * 1024 * 1024]).unwrap());
            io::stderr().write_all(&vec![b'e'; 9 * 1024 * 1024]).unwrap();
            out.join().unwrap();
            ExitCode::SUCCESS
        }
        "binary" => {
            io::stdout().write_all(b"\xff\x00\xfe\n").unwrap();
            io::stderr().write_all(b"\x01\x00text\n").unwrap();
            ExitCode::SUCCESS
        }
        "tree" => {
            std::process::Command::new(std::env::current_exe().unwrap()).arg("worker").spawn().unwrap();
            std::fs::write("ready", "ready").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(30));
            ExitCode::SUCCESS
        }
        "worker" => {
            std::thread::sleep(std::time::Duration::from_millis(1200));
            std::fs::write("escaped", "escaped").unwrap();
            ExitCode::SUCCESS
        }
        _ => panic!("unknown fixture mode"),
    }
}
