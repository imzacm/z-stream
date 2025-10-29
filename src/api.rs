use crate::stream::Command;

pub fn start_api_task(port: u16, command_tx: flume::Sender<Command>) {
    let server = tiny_http::Server::http(("0.0.0.0", port)).expect("Failed to start server");

    std::thread::spawn(move || {
        loop {
            let request = match server.recv() {
                Ok(request) => request,
                Err(error) => {
                    eprintln!("Error: {error}");
                    break;
                }
            };

            handle_request(request, command_tx.clone());
        }
    });
}

fn handle_request(request: tiny_http::Request, command_tx: flume::Sender<Command>) {
    let method = request.method();
    let path = request.url();
    eprintln!("Request: {method} {path}");
    if *method == tiny_http::Method::Get && path == "/skip" {
        _ = command_tx.send(Command::Skip);
    }
    let response = tiny_http::Response::empty(200);
    _ = request.respond(response);
}
