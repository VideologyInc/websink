/* eslint-env browser */

let pc = new RTCPeerConnection({
  iceServers: [
    {
      urls: 'stun:stun.l.google.com:19302'
    }
  ]
})

// Update connection status display
function showStatus(status, error = false) {
  const statusElement = document.getElementById('connectionStatus');
  statusElement.textContent = status;
  if (error) {
    statusElement.style.backgroundColor = 'rgba(255, 0, 0, 0.5)';
  } else {
    statusElement.style.backgroundColor = 'rgba(  0, 0, 0, 0.4)';
  }
}

function checkCodecCompatibility(receivedCodec) {
  const codecs = RTCRtpReceiver.getCapabilities('video').codecs;
  const codecString = codecs.map(c => c.mimeType).join(' ').toLowerCase();

  if (!codecString.includes(receivedCodec.toLowerCase())) {
    showStatus(`❌ This browser does not support ${receivedCodec.toUpperCase()} video codec. Try Safari for H.265 support.`, true);
    return false;
  }
  return true;
}

pc.ontrack = function (event) {
  if (event.track.kind === 'video') {
    var videoElement = document.createElement('video');
    videoElement.srcObject = event.streams[0];
    videoElement.autoplay = true;
    videoElement.playsInline = true;
    videoElement.muted = true;

    // Add loadedmetadata event listener to ensure video is ready
    videoElement.addEventListener('loadedmetadata', function() {
      // Ensure playback starts
      videoElement.play().catch(e => {
        console.warn(`Autoplay failed: ${e.message}. User interaction may be required.`);
      });
    });

    // Clear any existing video elements
    const videoContainer = document.getElementById('videoContainer');
    videoContainer.innerHTML = '';
    videoContainer.appendChild(videoElement);
  }
}

let iceTimout = null

function sendOffer() {
  (async () => {
    try {
      console.log('Sending offer...');

      const res = await fetch('/api/session', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ offer: pc.localDescription }),
      });

      const contentType = res.headers.get('content-type') || '';
      const text = await res.text();

      if (!res.ok) {
        console.error(`❌ Offer declined: ${res.status} ${text}`);
        showStatus(`Connection failed: ${text || res.statusText}`, true);
        return;
      }

      const data = contentType.includes('application/json') && text ? JSON.parse(text) : {};
      console.log('Received answer from server');

      if (data.negotiated_codec) {
        console.log(`Server is sending: ${data.negotiated_codec.toUpperCase()}`);
        if (!checkCodecCompatibility(data.negotiated_codec)) return;
        showStatus(`Receiving: ${data.negotiated_codec.toUpperCase()}`);
      }

      await pc.setRemoteDescription(data.answer);
    } catch (err) {
      console.error(`❌ Connection failed: ${err.message}`);
      showStatus(`Connection failed: ${err.message}`, true);
    }
  })();
}

// Function to close the connection gracefully
function closeConnection() {
  console.log('Closing connection...');

  // Close the RTCPeerConnection
  // This will send the appropriate signals to the server
  if (pc && pc.connectionState !== 'closed') {
    // Close the peer connection
    pc.close();
    showStatus('Disconnected');
    console.log('Connection closed');
  }
}

pc.oniceconnectionstatechange = e => {
  const state = pc.iceConnectionState;
  console.log(`Connection: ${state}`);

  // When connection is established, try to play all video elements
  if (state === 'connected' || state === 'completed') {
    showStatus('Connected');
    // Find all video elements and try to play them
    document.querySelectorAll('video').forEach(el => {
      el.play().catch(e => {
        console.warn(`Autoplay failed: ${e.message}`);
      });
      showStatus('Playing');
    });
  }
}

pc.onicecandidate = event => {
  if (event.candidate === null) {
    // ICE gathering is complete, we can now send the offer to the server
    console.log('ICE gathering complete');
  } else {
    // fire after 150ms of no new candidates
    if (iceTimout) {
      clearTimeout(iceTimout)
    }
    iceTimout = setTimeout(sendOffer, 150)
  }
}

// Initialize connection
console.log('Starting connection...');

// Offer to receive video track only
const audioTransceiver = pc.addTransceiver('audio', { direction: 'recvonly' });
const videoTransceiver = pc.addTransceiver('video', { direction: 'recvonly' });

pc.createOffer()
   .then(offer => {
       console.log('Offer created');
       return pc.setLocalDescription(offer);
   })
   .catch(error => {
       console.log('Error creating offer');
       console.error(`Error: ${error.message}`);
   });

// Add event listener for page unload
window.addEventListener('beforeunload', function(event) {
  closeConnection();
});

// Add event listener for page visibility change
// document.addEventListener('visibilitychange', function() {
//   if (document.visibilityState === 'hidden') {
//     // Page is hidden (tab switched, minimized, etc.)
//     // This can help in some cases where beforeunload doesn't fire
//     closeConnection();
//   }
// });
