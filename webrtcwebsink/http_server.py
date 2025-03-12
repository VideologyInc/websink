import os
import json
import logging
from http.server import SimpleHTTPRequestHandler
from typing import Union, Tuple
from urllib.parse import urlparse

# Configure logging
logger = logging.getLogger('webrtcwebsink.server')

class WebRTCHTTPHandler(SimpleHTTPRequestHandler):
    """Custom HTTP handler for serving the WebRTC client files."""

    ws_port = 8081  # Default WebSocket port

    def __init__(self, *args, **kwargs):
        # Get the directory containing this file
        current_dir = os.path.dirname(os.path.abspath(__file__))
        self.static_dir = os.path.join(current_dir, 'static')
        super().__init__(*args, directory=self.static_dir, **kwargs)

    def translate_path(self, path: str) -> str:
        """Translate URL path to filesystem path."""
        # Parse the path
        parsed_path = urlparse(path).path

        # Default to index.html for root path
        if parsed_path == '/':
            parsed_path = '/index.html'

        # Remove leading slash and join with static directory
        clean_path = parsed_path.lstrip('/')
        return os.path.join(self.static_dir, clean_path)

    def do_GET(self):
        """Handle GET requests."""
        try:
            # Handle API endpoints
            if self.path == '/api/config':
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Cache-Control', 'no-cache')
                self.end_headers()

                # Send WebSocket port as JSON
                config = {
                    'ws_port': self.ws_port
                }
                self.wfile.write(json.dumps(config).encode('utf-8'))
                return

            # Get the filesystem path
            file_path = self.translate_path(self.path)

            # Check if file exists
            if not os.path.exists(file_path):
                self.send_error(404, "File not found")
                return

            # Serve the file
            self.path = '/' + os.path.relpath(file_path, self.static_dir)
            super().do_GET()

        except Exception as e:
            logger.error(f"Server Error: {e}")
            self.send_error(500, str(e))

    def log_message(self, format: str, *args: any) -> None:
        """Override to provide more useful logging."""
        logger.info(f"{format%args}")

    def guess_type(self, path) -> str:
        """Guess the type of a file based on its extension."""
        base, ext = os.path.splitext(path)

        if ext == '.js':
            return 'application/javascript'
        elif ext == '.html':
            return 'text/html'
        elif ext == '.css':
            return 'text/css'

        return super().guess_type(path)
