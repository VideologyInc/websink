#!/bin/bash
set -e

# Remove any existing installation
pip uninstall -y webrtcwebsink || true

# Install build dependencies
pip install --upgrade pip build

# Build and install the package
pip install --use-pep517 -e .

# Run the test application
echo "Running test application..."
echo "Once running, open your web browser to http://localhost:8080"
echo "Press Ctrl+C to stop"
python3 test_webrtcwebsink.py