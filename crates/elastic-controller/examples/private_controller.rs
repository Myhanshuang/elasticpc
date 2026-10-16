//! Dedicated process entry for the explicitly authorized private laboratory.
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if let Err(error) = elastic_controller::private_libvirt_lab::run_worker(&args) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
