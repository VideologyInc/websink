// WebRTC and WebSocket connections
let pc = null;
let ws = null;

// Expose peerConnection globally for testing
window.peerConnection = null;

// DOM elements
const statusElement = document.getElementById('status');
const videoElement = document.getElementById('stream');
const errorElement = document.getElementById('error');

// Initialize video settings
function checkElements() {
    if (!statusElement || !videoElement) {
        console.error('Required DOM elements not found');
        return false;
    }
    return true;
}

function setStatus(text) {
    if (statusElement) {
        statusElement.textContent = text;
    }
    console.log('Status:', text);
}

function showError(text) {
    console.error('Error:', text);
    setStatus('Error: ' + text);
    if (errorElement) {
        errorElement.textContent = text;
        errorElement.style.display = 'block';
    }
}

function initVideo() {
    videoElement.playsInline = true;
    videoElement.autoplay = true;
    videoElement.muted = true;  // Mute to allow autoplay
}

// Detect supported codecs
async function detectSupportedCodecs() {
    try {
        const codecs = [
            { name: 'h264', mimeType: 'video/h264' },
            { name: 'vp8', mimeType: 'video/VP8' },
            { name: 'hevc', mimeType: 'video/hevc' },
            { name: 'av1', mimeType: 'video/AV1' }
        ];

        // Check which codecs are supported
        const supportedCodecs = [];
        for (const codec of codecs) {
            if (RTCRtpReceiver.getCapabilities) {
                const capabilities = RTCRtpReceiver.getCapabilities('video');
                const supported = capabilities.codecs.some(c =>
                    c.mimeType.toLowerCase() === codec.mimeType.toLowerCase());
                if (supported) {
                    supportedCodecs.push(codec.name);
                }
            } else {
                // Fallback for browsers without getCapabilities
                supportedCodecs.push(codec.name);
            }
        }
        console.log('Supported codecs:', supportedCodecs);

        // Prefer h264 if supported, otherwise use the first supported codec
        return supportedCodecs;
    } catch (error) {
        return [];
    }
}

function createPeerConnection() {
    if (pc) {
        console.log('Closing existing peer connection');
        pc.close();
    }

    const config = {
        iceServers: [{
            urls: 'stun:stun.l.google.com:19302'
        }]
    };

    pc = new RTCPeerConnection(config);
    // Set global reference for testing
    window.peerConnection = pc;
    console.log('Created new peer connection');

    // Set up video handling
    pc.ontrack = function(event) {
        console.log('Received track:', event.track.kind);
        if (event.track.kind === 'video') {
            console.log('Setting up video track');
            videoElement.srcObject = event.streams[0];
            console.log('Video stream set');

            event.track.onunmute = () => {
                console.log('Video track unmuted');
                videoElement.play()
                    .then(() => {
                        console.log('Video playing');
                        setStatus('Video streaming');
                    })
                    .catch(error => {
                        console.error('Error playing video:', error);
                        showError('Failed to play video: ' + error.message);
                    });
            };

            event.track.onended = () => {
                console.log('Video track ended');
                setStatus('Video stream ended');
                videoElement.srcObject = null;
            };
        }
    };

    // Connection monitoring
    // Store last error for debugging
    window.lastWebRTCError = null;

    pc.onconnectionstatechange = () => {
        console.log('Connection state changed:', pc.connectionState);
        setStatus('Connection: ' + pc.connectionState);

        // Log detailed state
        console.log('Detailed WebRTC State:', {
            connectionState: pc.connectionState,
            iceConnectionState: pc.iceConnectionState,
            iceGatheringState: pc.iceGatheringState,
            signalingState: pc.signalingState,
            hasLocalDescription: !!pc.localDescription,
            hasRemoteDescription: !!pc.remoteDescription
        });

        if (pc.connectionState === 'failed') {
            window.lastWebRTCError = 'Connection failed';
            console.error('WebRTC connection failed');
            // Log transceivers state
            pc.getTransceivers().forEach((transceiver, idx) => {
                console.log(`Transceiver ${idx}:`, {
                    currentDirection: transceiver.currentDirection,
                    direction: transceiver.direction,
                    mid: transceiver.mid,
                    stopped: transceiver.stopped
                });
            });
        }
    };

    pc.oniceconnectionstatechange = () => {
        console.log('ICE connection state:', pc.iceConnectionState);
        setStatus('ICE connection: ' + pc.iceConnectionState);

        if (pc.iceConnectionState === 'failed') {
            window.lastWebRTCError = 'ICE connection failed';
            console.error('ICE connection failed');
            // Get ICE candidates
            pc.getStats().then(stats => {
                stats.forEach(report => {
                    if (report.type === 'candidate-pair' ||
                        report.type === 'local-candidate' ||
                        report.type === 'remote-candidate') {
                        console.log(`ICE Candidate Stats (${report.type}):`, report);
                    }
                });
            });
        }
    };

    pc.onicegatheringstatechange = () => {
        console.log('ICE gathering state:', pc.iceGatheringState);
        if (pc.iceGatheringState === 'complete') {
            console.log('ICE gathering complete');
        }
    };

    pc.onsignalingstatechange = () => {
        console.log('Signaling state:', pc.signalingState);
        if (pc.signalingState === 'stable') {
            console.log('Signaling state is stable');
        }
    };

    pc.onicecandidate = (event) => {
        if (event.candidate) {
            console.log('Sending ICE candidate');
            ws.send(`ROOM_PEER_MSG server ${JSON.stringify({
                iceCandidate: event.candidate
            })}`);
        }
    };

    // Add transceiver in receive-only mode
    pc.addTransceiver('video', {direction: 'recvonly'});

    return pc;
}

async function start() {
    try {
        // Check elements and initialize video
        if (!checkElements()) {
            throw new Error('Required DOM elements not found');
        }
        initVideo();

        console.log('Starting WebRTC connection...');

        // Fetch configuration from server
        let wsPort;
        try {
            const response = await fetch('/api/config');
            if (!response.ok) {
                throw new Error(`HTTP error! status: ${response.status}`);
            }
            const config = await response.json();
            wsPort = config.ws_port;
            console.log('Fetched WebSocket port from server:', wsPort);
        } catch (error) {
            console.warn('Failed to fetch config, using default port:', error);
            wsPort = 8081; // Fallback to default port
        }

        // Connect to signaling server
        const wsUrl = `ws://${window.location.hostname}:${wsPort}`;
        console.log('Connecting to WebSocket server at:', wsUrl);
        ws = new WebSocket(wsUrl);

        ws.onopen = function() {
            console.log('WebSocket connection opened, sending HELLO');
            ws.send('HELLO');
            setStatus('Connecting to signaling server...');
        };

        ws.onclose = function(event) {
            console.log('WebSocket connection closed:', event.code, event.reason);
            setStatus('WebSocket disconnected');
            if (pc) {
                console.log('Closing peer connection');
                pc.close();
                pc = null;
            }
        };

        ws.onerror = function(error) {
            console.error('WebSocket error:', error);
            showError('WebSocket connection failed');
        };

        ws.onmessage = async function(event) {
            try {
                console.log('Received message:', event.data);
                const msg = event.data;

                if (msg === 'HELLO') {
                    console.log('Got HELLO from signaling server. Joining ROOM webrtc...');
                    ws.send('ROOM webrtc');
                    setStatus('Joining WebRTC room...');
                    return;
                }

                if (msg.startsWith('ROOM_OK')) {
                    console.log('Got ROOM OK from signaling server');
                    setStatus('Joined WebRTC room, waiting for stream...');

                    // Detect and send preferred codec
                    try {
                        const supportedCodecs = await detectSupportedCodecs();
                        ws.send(`CODEC ${JSON.stringify(supportedCodecs)}`);
                    } catch (error) {
                        console.warn('Error sending codec preference:', error);
                    }
                    return;
                }

                if (msg.startsWith('ROOM_PEER_MSG')) {
                    const jsonStart = msg.indexOf('{');
                    if (jsonStart === -1) {
                        console.warn('Invalid ROOM_PEER_MSG format:', msg);
                        return;
                    }

                    let data;
                    try {
                        data = JSON.parse(msg.slice(jsonStart));
                        console.log('Parsed peer message:', data);
                    } catch (error) {
                        console.error('Error parsing peer message:', error);
                        return;
                    }

                    if (data.offer) {
                        try {
                            console.log('Received offer:', data.offer);
                            setStatus('Received offer, creating answer...');

                            if (!pc) {
                                console.log('Creating new peer connection');
                                pc = createPeerConnection();
                            }

                            // Check signaling state before proceeding
                            if (pc.signalingState !== 'stable') {
                                console.log('Signaling state not stable:', pc.signalingState);
                                console.log('Waiting for signaling state to stabilize...');
                                await new Promise(resolve => setTimeout(resolve, 1000));
                            }

                            console.log('Setting remote description with offer:', data.offer);
                            console.log('Received offer SDP:', data.offer.sdp);
                            await pc.setRemoteDescription(new RTCSessionDescription(data.offer));
                            console.log('Remote description set successfully');

                            // Log connection state after setting remote description
                            console.log('Connection states after remote description:', {
                                connectionState: pc.connectionState,
                                iceConnectionState: pc.iceConnectionState,
                                signalingState: pc.signalingState,
                                iceGatheringState: pc.iceGatheringState
                            });

                            console.log('Creating answer...');
                            const answer = await pc.createAnswer({
                                offerToReceiveVideo: true
                            });
                            console.log('Answer SDP:', answer.sdp);

                            console.log('Setting local description...');
                            await pc.setLocalDescription(answer);
                            console.log('Local description set successfully');

                            // Log connection state after setting local description
                            console.log('Connection states after local description:', {
                                connectionState: pc.connectionState,
                                iceConnectionState: pc.iceConnectionState,
                                signalingState: pc.signalingState,
                                iceGatheringState: pc.iceGatheringState
                            });

                            // Log transceivers state
                            // pc.getTransceivers().forEach((transceiver, idx) => {
                            //     console.log(`Transceiver ${idx}:`, {
                            //         currentDirection: transceiver.currentDirection,
                            //         direction: transceiver.direction,
                            //         mid: transceiver.mid,
                            //         sender: transceiver.sender.track ? transceiver.sender.track.kind : 'no track',
                            //         receiver: transceiver.receiver.track ? transceiver.receiver.track.kind : 'no track'
                            //     });
                            // });

                            // Send answer to server
                            ws.send(`ROOM_PEER_MSG server ${JSON.stringify({
                                answer: answer
                            })}`);
                            console.log('Answer sent to server');
                        } catch (error) {
                            console.error('Error during offer/answer:', error);
                            console.error('Error details:', {
                                name: error.name,
                                message: error.message,
                                stack: error.stack
                            });
                            showError('Error during offer/answer: ' + error.message);
                        }
                    } else if (data.iceCandidate) {
                        try {
                            console.log('Received ICE candidate:', data.iceCandidate);
                            if (pc) {
                                // Log ICE states before adding candidate
                                console.log('ICE states before adding candidate:', {
                                    iceConnectionState: pc.iceConnectionState,
                                    iceGatheringState: pc.iceGatheringState
                                });

                                await pc.addIceCandidate(data.iceCandidate);
                                console.log('Added ICE candidate successfully');

                                // Log ICE states after adding candidate
                                console.log('ICE states after adding candidate:', {
                                    iceConnectionState: pc.iceConnectionState,
                                    iceGatheringState: pc.iceGatheringState
                                });

                                // Log all current ICE candidates
                                const stats = await pc.getStats();
                                stats.forEach(report => {
                                    if (report.type === 'candidate-pair' || report.type === 'local-candidate' || report.type === 'remote-candidate') {
                                        console.log(`ICE Candidate Stats (${report.type}):`, report);
                                    }
                                });
                            } else {
                                console.warn('Received ICE candidate but no peer connection exists');
                            }
                        } catch (error) {
                            console.error('Error adding ICE candidate:', error);
                            console.error('Error details:', {
                                name: error.name,
                                message: error.message,
                                stack: error.stack
                            });
                        }
                    }
                }
            } catch (error) {
                console.error('Error handling message:', error);
                showError(error.message);
            }
        };
    } catch (error) {
        console.error('Error:', error);
        showError(error.message);
    }
}

// Start when page loads
window.addEventListener('load', start);