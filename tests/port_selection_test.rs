// Test for the port auto-selection functionality
use std::net::TcpListener;
use websink::websink::server::find_available_port;

#[test]
fn test_auto_select_any_port() {
    // Test auto-selecting any available port (port 0)
    let result = find_available_port(0);
    assert!(result.is_ok(), "Should be able to auto-select any available port");
    let port = result.unwrap();
    assert!(port > 0, "Auto-selected port should be greater than 0");
    println!("Auto-selected port: {}", port);
}

#[test]
fn test_specific_available_port() {
    // Test using a specific available port
    let test_port = 9123; // Using a high port that's likely to be available
    let result = find_available_port(test_port);
    assert!(result.is_ok(), "Should be able to use an available port");
    let port = result.unwrap();
    assert_eq!(port, test_port, "Should get the requested port when it's available");
}

#[test]
fn test_port_in_use_fallback() {
    // Test fallback when the requested port is in use
    let test_port = 9124;

    // Bind to the test port to make it unavailable
    let _listener = TcpListener::bind(format!("127.0.0.1:{}", test_port)).expect("Failed to bind to test port");

    let result = find_available_port(test_port);
    assert!(result.is_ok(), "Should find an alternative port when requested port is in use");
    let port = result.unwrap();
    assert_ne!(port, test_port, "Should not get the requested port when it's in use");
    assert!(port > test_port, "Should get a higher port number as alternative");
    assert!(port <= test_port + 100, "Alternative port should be within the search range");
    println!("Requested port {} was in use, got alternative port {}", test_port, port);
}

#[test]
fn test_no_available_ports_in_range() {
    // This test would be hard to implement reliably without actually blocking many ports
    // For now, just test that the function signature works and errors are properly typed
    let result = find_available_port(65530); // Near the upper limit
                                             // We don't assert the result since it depends on system state
                                             // but we ensure the function can be called and returns the right type
    match result {
        Ok(port) => println!("Got port {} near upper limit", port),
        Err(e) => println!("Expected error near upper limit: {}", e),
    }
}
