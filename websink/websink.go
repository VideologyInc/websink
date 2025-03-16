// SPDX-FileCopyrightText: 2023 The Pion community <https://pion.ly>
// SPDX-License-Identifier: MIT

//go:build !js
// +build !js

// gstreamer-send is a simple application that shows how to send video to your browser using Pion WebRTC and GStreamer.
package main

import (
	"embed"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"sync"

	"github.com/go-gst/go-gst/gst"
	"github.com/go-gst/go-gst/gst/app"
	"github.com/pion/webrtc/v4"
	"github.com/pion/webrtc/v4/pkg/media"

	_ "github.com/motemen/go-loghttp/global" // Just this line!
)

// SessionRequest represents the JSON structure for session requests
type SessionRequest struct {
	Offer json.RawMessage `json:"offer"`
}

// SessionResponse represents the JSON structure for session responses
type SessionResponse struct {
	Answer json.RawMessage `json:"answer"`
}

// Global variables to store shared tracks
var (
	// Mutex to protect access to the tracks
	tracksMutex sync.RWMutex

	// Shared video track
	videoTrack *webrtc.TrackLocalStaticSample

	// WebRTC configuration
	webrtcConfig webrtc.Configuration

	// Map to store active peer connections
	peerConnectionsMutex sync.RWMutex
	peerConnections      map[string]*webrtc.PeerConnection
)

//go:embed static/*
var static_files embed.FS

// updatePeerConnections adds or removes a peer connection from the global map
// and prints the current count
func updatePeerConnections(id string, pc *webrtc.PeerConnection, add bool) {
	peerConnectionsMutex.Lock()
	defer peerConnectionsMutex.Unlock()

	if add {
		peerConnections[id] = pc
	} else {
		delete(peerConnections, id)
	}

	fmt.Printf("Client count changed: %d connected clients\n", len(peerConnections))
}

// findAvailablePort finds an available port starting from the given port
func findAvailablePort(startPort int) (int, error) {
	port := startPort
	maxPort := startPort + 100 // Try up to 100 ports

	for port < maxPort {
		addr := fmt.Sprintf(":%d", port)
		listener, err := net.Listen("tcp", addr)
		if err == nil {
			listener.Close()
			return port, nil
		}
		port++
	}

	return 0, fmt.Errorf("no available ports found between %d and %d", startPort, maxPort)
}

func main() {
	videoSrc := flag.String("video-src", "videotestsrc", "GStreamer video src")
	defaultPort := flag.Int("port", 8082, "HTTP server port")
	flag.Parse()

	// Initialize GStreamer
	gst.Init(nil)

	// Prepare the configuration
	webrtcConfig = webrtc.Configuration{}

	// Initialize the peer connections map
	peerConnections = make(map[string]*webrtc.PeerConnection)

	// Create shared video track
	var err error
	videoTrack, err = webrtc.NewTrackLocalStaticSample(webrtc.RTPCodecCapability{MimeType: "video/h264"}, "video", "pion2")
	if err != nil {
		panic(err)
	}

	// Start pushing buffers on the video track
	go pipelineForCodec("h264", []*webrtc.TrackLocalStaticSample{videoTrack}, *videoSrc)

	// Set up HTTP handlers
	m := http.NewServeMux()
	fileserver := http.FileServer(http.Dir("./static"))

	m.HandleFunc("POST /api/session", handleSession)
	m.Handle("GET /favicon.ico", fileserver)
	m.Handle("GET /", fileserver)

	// Find an available port
	port, err := findAvailablePort(*defaultPort)
	if err != nil {
		panic(err)
	}

	// Print initial client count
	fmt.Printf("Client count: %d connected clients\n", len(peerConnections))

	// Start the HTTP server
	serverAddr := fmt.Sprintf(":%d", port)
	fmt.Printf("Server started at http://localhost:%d\n", port)
	fmt.Println("Multiple client connections are now supported")

	http.ListenAndServe(serverAddr, m)
	// Block forever
	select {}
}

// handleSession creates a handler for the /api/session endpoint
func handleSession(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	// Read the request body
	body, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, "Error reading request body", http.StatusBadRequest)
		return
	}

	// Parse the JSON request
	var sessionReq SessionRequest
	if err := json.Unmarshal(body, &sessionReq); err != nil {
		http.Error(w, "Error parsing JSON", http.StatusBadRequest)
		return
	}

	// Generate a unique ID for this peer connection
	peerID := fmt.Sprintf("peer-%d", len(peerConnections)+1)

	// Create a new peer connection for this client
	peerConnection, err := createPeerConnection(peerID)
	if err != nil {
		http.Error(w, "Error creating peer connection: "+err.Error(), http.StatusInternalServerError)
		return
	}

	// Add to the peer connections map
	updatePeerConnections(peerID, peerConnection, true)

	// Decode the offer
	offer := webrtc.SessionDescription{}
	if err := json.Unmarshal(sessionReq.Offer, &offer); err != nil {
		http.Error(w, "Error parsing offer: "+err.Error(), http.StatusBadRequest)
		// Remove from peer connections map if we fail
		updatePeerConnections(peerID, nil, false)
		return
	}

	// Set the remote SessionDescription
	if err := peerConnection.SetRemoteDescription(offer); err != nil {
		http.Error(w, "Error setting remote description: "+err.Error(), http.StatusInternalServerError)
		// Remove from peer connections map if we fail
		updatePeerConnections(peerID, nil, false)
		return
	}

	// Create an answer
	answer, err := peerConnection.CreateAnswer(nil)
	if err != nil {
		http.Error(w, "Error creating answer: "+err.Error(), http.StatusInternalServerError)
		// Remove from peer connections map if we fail
		updatePeerConnections(peerID, nil, false)
		return
	}

	// Sets the LocalDescription, and starts our UDP listeners
	if err = peerConnection.SetLocalDescription(answer); err != nil {
		http.Error(w, "Error setting local description: "+err.Error(), http.StatusInternalServerError)
		// Remove from peer connections map if we fail
		updatePeerConnections(peerID, nil, false)
		return
	}

	// Wait for ICE gathering to complete
	gatherComplete := webrtc.GatheringCompletePromise(peerConnection)
	<-gatherComplete

	// Marshal the answer to JSON
	answerJSON, err := json.Marshal(peerConnection.LocalDescription())
	if err != nil {
		http.Error(w, "Error encoding answer: "+err.Error(), http.StatusInternalServerError)
		// Remove from peer connections map if we fail
		updatePeerConnections(peerID, nil, false)
		return
	}

	// Return the answer as JSON
	w.Header().Set("Content-Type", "application/json")
	response := SessionResponse{
		Answer: answerJSON,
	}
	json.NewEncoder(w).Encode(response)
}

// createPeerConnection creates a new peer connection with the shared tracks
func createPeerConnection(peerID string) (*webrtc.PeerConnection, error) {
	// Create a new RTCPeerConnection
	peerConnection, err := webrtc.NewPeerConnection(webrtcConfig)
	if err != nil {
		return nil, err
	}

	// Set the handler for ICE connection state
	// This will notify you when the peer has connected/disconnected
	peerConnection.OnICEConnectionStateChange(func(connectionState webrtc.ICEConnectionState) {
		fmt.Printf("Connection State for %s has changed to %s\n", peerID, connectionState.String())

		// Clean up when disconnected
		if connectionState == webrtc.ICEConnectionStateDisconnected ||
			connectionState == webrtc.ICEConnectionStateFailed ||
			connectionState == webrtc.ICEConnectionStateClosed {
			fmt.Printf("Peer %s disconnected, cleaning up\n", peerID)
			// Remove from peer connections map
			updatePeerConnections(peerID, nil, false)
			// Close the peer connection to free resources
			peerConnection.Close()
		}
	})

	// Lock to safely access the shared tracks
	tracksMutex.RLock()
	defer tracksMutex.RUnlock()

	// Add the video track to the peer connection
	_, err = peerConnection.AddTrack(videoTrack)
	if err != nil {
		return nil, err
	}

	return peerConnection, nil
}

// Create the appropriate GStreamer pipeline depending on what codec we are working with
func pipelineForCodec(codecName string, tracks []*webrtc.TrackLocalStaticSample, pipelineSrc string) {
	pipelineStr := "appsink name=appsink"
	switch codecName {
	case "vp8":
		pipelineStr = pipelineSrc + " ! vp8enc error-resilient=partitions keyframe-max-dist=10 auto-alt-ref=true cpu-used=5 deadline=1 ! " + pipelineStr
	case "vp9":
		pipelineStr = pipelineSrc + " ! vp9enc ! " + pipelineStr
	case "h264":
		pipelineStr = pipelineSrc + " ! video/x-raw,format=I420 ! x264enc speed-preset=ultrafast tune=zerolatency key-int-max=20 ! video/x-h264,stream-format=byte-stream ! " + pipelineStr
	default:
		panic("Unhandled codec " + codecName) //nolint
	}

	pipeline, err := gst.NewPipelineFromString(pipelineStr)
	if err != nil {
		panic(err)
	}

	if err = pipeline.SetState(gst.StatePlaying); err != nil {
		panic(err)
	}

	appSink, err := pipeline.GetElementByName("appsink")
	if err != nil {
		panic(err)
	}

	app.SinkFromElement(appSink).SetCallbacks(&app.SinkCallbacks{
		NewSampleFunc: func(sink *app.Sink) gst.FlowReturn {
			sample := sink.PullSample()
			if sample == nil {
				return gst.FlowEOS
			}

			buffer := sample.GetBuffer()
			if buffer == nil {
				return gst.FlowError
			}

			samples := buffer.Map(gst.MapRead).Bytes()
			defer buffer.Unmap()

			for _, t := range tracks {
				if err := t.WriteSample(media.Sample{Data: samples, Duration: *buffer.Duration().AsDuration()}); err != nil {
					fmt.Printf("Error writing sample to track: %v\n", err)
					// Don't panic here, just log the error and continue
				}
			}

			return gst.FlowOK
		},
	})
}
