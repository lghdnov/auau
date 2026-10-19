import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, 'fixtures', 'sine3s-8khz.wav');

const FREQ_HZ = 440;
const SECS = 3.0;
const SAMPLE_RATE = 8000;
const AMPLITUDE = 0.9 * 32767.0;

const sampleCount = Math.round(SECS * SAMPLE_RATE);
const dataLen = sampleCount * 2;

const buf = Buffer.alloc(44 + dataLen);
buf.write('RIFF', 0, 'ascii');
buf.writeUInt32LE(36 + dataLen, 4);
buf.write('WAVE', 8, 'ascii');
buf.write('fmt ', 12, 'ascii');
buf.writeUInt32LE(16, 16);
buf.writeUInt16LE(1, 20);
buf.writeUInt16LE(1, 22);
buf.writeUInt32LE(SAMPLE_RATE, 24);
buf.writeUInt32LE(SAMPLE_RATE * 2, 28);
buf.writeUInt16LE(2, 32);
buf.writeUInt16LE(16, 34);
buf.write('data', 36, 'ascii');
buf.writeUInt32LE(dataLen, 40);

for (let n = 0; n < sampleCount; n++) {
	const t = n / SAMPLE_RATE;
	const sample = Math.trunc(AMPLITUDE * Math.sin(2 * Math.PI * FREQ_HZ * t));
	buf.writeInt16LE(sample, 44 + n * 2);
}

mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, buf);
console.log(`wrote ${out} (${buf.length} bytes, ${sampleCount} samples)`);
