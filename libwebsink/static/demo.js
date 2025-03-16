// SPDX-FileCopyrightText: 2023 The Pion community <https://pion.ly>
// SPDX-License-Identifier: MIT

/* eslint-env browser */

let pc = new RTCPeerConnection({
  iceServers: [
    {
      urls: 'stun:stun.l.google.com:19302'
    }
  ]
})

// Update connection status display
function updateConnectionStatus(status) {
  const statusElement = document.getElementById('connectionStatus');
  statusElement.textContent = status;
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
  updateConnectionStatus('Sending offer...');

  // Send the offer to the server as JSON directly
  fetch('/api/session', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      offer: pc.localDescription
    }),
  })
  .then(response => {
    if (!response.ok) {
      throw new Error(`Server responded with ${response.status}: ${response.statusText}`)
    }
    return response.json()
  })
  .then(data => {
    updateConnectionStatus('Received answer');

    // Set the remote description with the answer from the server
    pc.setRemoteDescription(data.answer);
  })
  .catch(error => {
    updateConnectionStatus('Connection failed');
    console.error(`Error: ${error.message}`);
  });
}

// Function to close the connection gracefully
function closeConnection() {
  updateConnectionStatus('Closing connection...');

  // Close the RTCPeerConnection
  // This will send the appropriate signals to the server
  if (pc && pc.connectionState !== 'closed') {
    // Close the peer connection
    pc.close();

    updateConnectionStatus('Connection closed');
  }
}

pc.oniceconnectionstatechange = e => {
  const state = pc.iceConnectionState;
  updateConnectionStatus(`Connection: ${state}`);

  // When connection is established, try to play all video elements
  if (state === 'connected' || state === 'completed') {
    updateConnectionStatus('Connected');
    // Find all video elements and try to play them
    document.querySelectorAll('video').forEach(el => {
      el.play().catch(e => {
        console.warn(`Autoplay failed: ${e.message}`);
      });
    });
  } else if (state === 'disconnected' || state === 'failed' || state === 'closed') {
    updateConnectionStatus('Disconnected');
  }
}

pc.onicecandidate = event => {
  if (event.candidate === null) {
    // ICE gathering is complete, we can now send the offer to the server
    updateConnectionStatus('ICE gathering complete');
  } else {
    // fire after 150ms of no new candidates
    if (iceTimout) {
      clearTimeout(iceTimout)
    }
    iceTimout = setTimeout(sendOffer, 150)
  }
}

// Initialize connection
updateConnectionStatus('Starting connection...');

// Offer to receive video track only
pc.addTransceiver('video', {'direction': 'sendrecv'})
pc.createOffer()
   .then(offer => {
       updateConnectionStatus('Offer created');
       return pc.setLocalDescription(offer);
   })
   .catch(error => {
       updateConnectionStatus('Error creating offer');
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
