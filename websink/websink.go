// SPDX-FileCopyrightText: 2023 The Pion community <https://pion.ly>
// SPDX-License-Identifier: MIT

//go:build !js
// +build !js

// gstreamer-send is a simple application that shows how to send video to your browser using Pion WebRTC and GStreamer.
package main

import (
	"embed"
	"encoding/base64"
	"encoding/json"
	"flag"
	"fmt"
	"io"
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
	Offer string `json:"offer"`
}

// Global variables to store shared tracks
var (
	// Mutex to protect access to the tracks
	tracksMutex sync.RWMutex

	// Shared tracks that will be sent to all clients
	audioTrack *webrtc.TrackLocalStaticSample
	videoTrack *webrtc.TrackLocalStaticSample

	// WebRTC configuration
	webrtcConfig webrtc.Configuration
)

//go:embed static/*
var static_files embed.FS

func main() {
	audioSrc := flag.String("audio-src", "audiotestsrc", "GStreamer audio src")
	videoSrc := flag.String("video-src", "videotestsrc", "GStreamer video src")
	flag.Parse()

	// Initialize GStreamer
	gst.Init(nil)

	// Prepare the configuration
	webrtcConfig = webrtc.Configuration{}

	// Create shared audio track
	var err error
	audioTrack, err = webrtc.NewTrackLocalStaticSample(webrtc.RTPCodecCapability{MimeType: "audio/opus"}, "audio", "pion1")
	if err != nil {
		panic(err)
	}

	// Create shared video track
	videoTrack, err = webrtc.NewTrackLocalStaticSample(webrtc.RTPCodecCapability{MimeType: "video/h264"}, "video", "pion2")
	if err != nil {
		panic(err)
	}

	// Start pushing buffers on these tracks
	go pipelineForCodec("opus", []*webrtc.TrackLocalStaticSample{audioTrack}, *audioSrc)
	go pipelineForCodec("h264", []*webrtc.TrackLocalStaticSample{videoTrack}, *videoSrc)

	// Set up HTTP handlers
	m := http.NewServeMux()
	fileserver := http.FileServer(http.Dir("./static"))

	m.HandleFunc("POST /api/session", handleSession)
	m.Handle("GET /favicon.ico", fileserver)
	m.Handle("GET /", fileserver)

	println("Server started at http://localhost:8082")
	println("Multiple client connections are now supported")

	http.ListenAndServe(":8082", m)
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

	// Create a new peer connection for this client
	peerConnection, err := createPeerConnection()
	if err != nil {
		http.Error(w, "Error creating peer connection: "+err.Error(), http.StatusInternalServerError)
		return
	}

	// Decode the offer
	offer := webrtc.SessionDescription{}
	decode(sessionReq.Offer, &offer)

	// Set the remote SessionDescription
	if err := peerConnection.SetRemoteDescription(offer); err != nil {
		http.Error(w, "Error setting remote description: "+err.Error(), http.StatusInternalServerError)
		return
	}

	// Create an answer
	answer, err := peerConnection.CreateAnswer(nil)
	if err != nil {
		http.Error(w, "Error creating answer: "+err.Error(), http.StatusInternalServerError)
		return
	}

	// Sets the LocalDescription, and starts our UDP listeners
	if err = peerConnection.SetLocalDescription(answer); err != nil {
		http.Error(w, "Error setting local description: "+err.Error(), http.StatusInternalServerError)
		return
	}

	// Wait for ICE gathering to complete
	gatherComplete := webrtc.GatheringCompletePromise(peerConnection)
	<-gatherComplete

	// Return the answer as JSON
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]string{
		"answer": encode(peerConnection.LocalDescription()),
	})
}

// createPeerConnection creates a new peer connection with the shared tracks
func createPeerConnection() (*webrtc.PeerConnection, error) {
	// Create a new RTCPeerConnection
	peerConnection, err := webrtc.NewPeerConnection(webrtcConfig)
	if err != nil {
		return nil, err
	}

	// Set the handler for ICE connection state
	// This will notify you when the peer has connected/disconnected
	peerConnection.OnICEConnectionStateChange(func(connectionState webrtc.ICEConnectionState) {
		fmt.Printf("Connection State has changed to %s\n", connectionState.String())

		// Clean up when disconnected
		if connectionState == webrtc.ICEConnectionStateDisconnected ||
			connectionState == webrtc.ICEConnectionStateFailed ||
			connectionState == webrtc.ICEConnectionStateClosed {
			fmt.Println("Peer disconnected, cleaning up")
			// Close the peer connection to free resources
			peerConnection.Close()
		}
	})

	// Lock to safely access the shared tracks
	tracksMutex.RLock()
	defer tracksMutex.RUnlock()

	// Add the audio track to the peer connection
	_, err = peerConnection.AddTrack(audioTrack)
	if err != nil {
		return nil, err
	}

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
	case "opus":
		// pipelineStr = pipelineSrc + " ! avenc_opus ! " + pipelineStr
		pipelineStr = pipelineSrc + " ! opusenc ! " + pipelineStr
	case "pcmu":
		pipelineStr = pipelineSrc + " ! audio/x-raw, rate=8000 ! mulawenc ! " + pipelineStr
	case "pcma":
		pipelineStr = pipelineSrc + " ! audio/x-raw, rate=8000 ! alawenc ! " + pipelineStr
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

// JSON encode + base64 a SessionDescription
func encode(obj *webrtc.SessionDescription) string {
	b, err := json.Marshal(obj)
	if err != nil {
		panic(err)
	}

	return base64.StdEncoding.EncodeToString(b)
}

// Decode a base64 and unmarshal JSON into a SessionDescription
func decode(in string, obj *webrtc.SessionDescription) {
	b, err := base64.StdEncoding.DecodeString(in)
	if err != nil {
		panic(err)
	}

	if err = json.Unmarshal(b, obj); err != nil {
		panic(err)
	}
}
