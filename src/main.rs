//#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::Path;

const PROCDUMP: &[u8; 1370432] = include_bytes!("../procdump.exe");

fn main() {
    let args = std::env::args_os().collect::<Vec<_>>();

    let current_exe = std::env::current_exe().expect("Failed to get current exe path");
    let current_dir = current_exe.parent().expect("Failed to get current dir");
    let dmp_dir = current_dir.join("dump_file");

    std::fs::create_dir_all(&dmp_dir).expect("Failed to create dir");

    if args.len() == 2 {
        let tmp = args[1].to_string_lossy();
        if ["--help", "-h", "help", "extract", "--extract"].contains(&&*tmp) {
            std::fs::write(dmp_dir.join("main.rs"), include_str!("./main.rs"))
                .expect("Error to extract source code.");
        }
        println!(
            "Assistant for creating dump file. Version: {}",
            env!("CARGO_PKG_VERSION")
        );
        return;
    }

    match running_processes() {
        Ok(processes) => get_dmp(&dmp_dir, &processes),
        Err(error) => eprintln!("Failed to list processes: {error}"),
    }

    pause("");
}

fn running_processes() -> std::io::Result<Vec<(&'static str, u32)>> {
    let process_list = std::process::Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()?;
    if !process_list.status.success() {
        return Err(std::io::Error::other(format!(
            "tasklist exited with {}: {}{}",
            process_list.status,
            String::from_utf8_lossy(&process_list.stdout),
            String::from_utf8_lossy(&process_list.stderr),
        )));
    }
    Ok(parse_process_list(&String::from_utf8_lossy(
        &process_list.stdout,
    )))
}

fn parse_process_list(process_list: &str) -> Vec<(&'static str, u32)> {
    process_list
        .lines()
        .filter_map(|line| {
            // Only the image name and PID are needed; later CSV fields can contain commas.
            let mut fields = line.splitn(3, ',');
            let image_name = fields.next()?.trim().trim_matches('"');
            let name = ["lertaro.app.exe", "lertaro.service.exe"]
                .into_iter()
                .find(|name| name.eq_ignore_ascii_case(image_name))?;
            let pid = fields.next()?.trim().trim_matches('"').parse().ok()?;
            Some((name, pid))
        })
        .collect()
}

fn get_dmp(dmp_dir: &Path, processes: &[(&str, u32)]) {
    if processes.is_empty() {
        println!("Not Running");
        return;
    }
    let dump_exe = dmp_dir.join("procdump.exe");
    std::fs::write(&dump_exe, PROCDUMP).expect("Error to extract procdump");

    let mut succeeded = 0;
    for (proc_name, pid) in processes {
        println!("Creating dump for {proc_name} (PID {pid})...");
        // A process name is ambiguous when several instances are running.
        let result = std::process::Command::new(&dump_exe)
            .args([
                "-accepteula",
                "-ma",
                "-o",
                &pid.to_string(),
                "PROCESSNAME_PID.dmp",
            ])
            .current_dir(dmp_dir)
            .status();
        match result {
            Ok(status) if status.success() => {
                succeeded += 1;
                println!("{proc_name} (PID {pid}) dump file created.");
            }
            Ok(status) => eprintln!(
                "Dump failed for {proc_name} (PID {pid}): {status}. See ProcDump output above."
            ),
            Err(error) => eprintln!("Failed to run ProcDump for {proc_name} (PID {pid}): {error}"),
        }
    }

    println!(
        "\n---DUMP FILES FINISHED: {succeeded} succeeded, {} failed---\nOutput: {}",
        processes.len() - succeeded,
        dmp_dir.display(),
    );
}

fn pause(str: impl ToString) {
    let mut input = str.to_string();
    input.push_str(
        format!(
            "{}Press Enter to continue...",
            if input.is_empty() { "" } else { "\n" }
        )
        .as_str(),
    );
    println!("{}", input);
    std::io::stdin().read_line(&mut input).unwrap();
}
