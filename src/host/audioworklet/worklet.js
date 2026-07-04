registerProcessor("CpalProcessor", class WasmProcessor extends AudioWorkletProcessor {
    constructor(options) {
        super();
        let [module, memory, handle] = options.processorOptions;
        bindgen.initSync({ module, memory });
        this.processor = bindgen.WasmAudioProcessor.unpack(handle);
        this.memory = memory;
        this.wasm_memory = new Float32Array(memory.buffer);
    }

    process(inputs, outputs) {
        // Check if memory grew and update view
        if (this.wasm_memory.buffer !== this.memory.buffer) {
            this.wasm_memory = new Float32Array(this.memory.buffer);
        }

        // Input node: it has an input port (inputs.length > 0). It is connected
        // to the destination with a silent output ONLY so the render graph
        // schedules its context (a node with no destination path is never
        // pulled, so its context may never start rendering). Read input if
        // present, hand it to Rust, and output silence.
        if (inputs.length > 0) {
            const input = inputs[0];
            if (input && input.length > 0) {
                const in_channels = input.length;
                const in_frame_size = input[0].length;
                const in_ptr = this.processor.input_buffer_ptr(in_channels, in_frame_size);
                const in_start = in_ptr / 4;
                const mem = this.wasm_memory;
                const in_total = in_channels * in_frame_size;
                if (in_start + in_total <= mem.length) {
                    for (let ch = 0; ch < in_channels; ch++) {
                        const channel = input[ch];
                        let write_pos = in_start + ch;
                        for (let i = 0; i < in_frame_size; i++) {
                            mem[write_pos] = channel[i];
                            write_pos += in_channels;
                        }
                    }
                    this.processor.process_input(in_channels, in_frame_size, sampleRate, currentTime);
                }
            }
            for (const out of outputs) for (const chan of out) chan.fill(0);
            return true;
        }

        const channels = outputs[0];
        const channels_count = channels.length;
        const frame_size = channels[0].length;
        const interleaved_ptr = this.processor.process(
            channels_count,
            frame_size,
            sampleRate,
            currentTime
        );

        const interleaved_start = interleaved_ptr / 4; // Convert byte offset to f32 index
        const interleaved = this.wasm_memory;

        const total_samples = frame_size * channels_count;
        if (interleaved_start + total_samples > this.wasm_memory.length) {
            console.error("CpalProcessor: Audio buffer out of bounds! Ptr:", interleaved_ptr, "Len:", total_samples);
            return false; // Safely stop the node
        }

        // Deinterleave: read strided from Wasm, write sequential to output
        for (let ch = 0; ch < channels_count; ch++) {
            const channel = channels[ch];
            let read_pos = interleaved_start + ch;

            for (let i = 0; i < frame_size; i++) {
                channel[i] = interleaved[read_pos];
                read_pos += channels_count;
            }
        }

        return true;
    }
});