/* A Windows program that makes sound as real ones do, for justOS's Wine
 * audio check: through WASAPI (mmdevapi, IAudioClient), through the old
 * waveOut of winmm, and recording through WASAPI's capture client.
 *
 *   sndtest wasapi [hz] [ms]
 *   sndtest waveout [hz] [ms]
 *   sndtest capture [ms] [file.wav]
 *
 * Each prints one line with what it did and how long it took.
 *
 * Build: x86_64-w64-mingw32-gcc -O2 -o sndtest.exe sndtest.c -lole32 -lwinmm -luuid -lm
 * (scripts/test-wine-audio.sh does). */
#define COBJMACROS
#define INITGUID
#include <windows.h>
#include <mmsystem.h>
#include <mmdeviceapi.h>
#include <audioclient.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const GUID SUBTYPE_FLOAT = {0x00000003, 0x0000, 0x0010, {0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71}};

static int is_float(const WAVEFORMATEX *f)
{
    if (f->wFormatTag == WAVE_FORMAT_IEEE_FLOAT)
        return 1;
    if (f->wFormatTag == WAVE_FORMAT_EXTENSIBLE)
        return IsEqualGUID(&((const WAVEFORMATEXTENSIBLE *)f)->SubFormat, &SUBTYPE_FLOAT);
    return 0;
}

/* Frames of a sine at hz into buf, in the format f, from frame `at`. */
static void fill(BYTE *buf, UINT32 frames, const WAVEFORMATEX *f, double hz, UINT64 at)
{
    UINT32 i, c;
    for (i = 0; i < frames; i++) {
        double v = 0.25 * sin(2 * M_PI * hz * (double)(at + i) / f->nSamplesPerSec);
        for (c = 0; c < f->nChannels; c++) {
            if (is_float(f))
                ((float *)buf)[i * f->nChannels + c] = (float)v;
            else if (f->wBitsPerSample == 16)
                ((short *)buf)[i * f->nChannels + c] = (short)(v * 32767);
            else if (f->wBitsPerSample == 32)
                ((int *)buf)[i * f->nChannels + c] = (int)(v * 2147483647.0);
        }
    }
}

static int fail(const char *what, HRESULT hr)
{
    printf("sndtest: %s failed: 0x%08lx\n", what, (unsigned long)hr);
    return 1;
}

static IAudioClient *open_client(EDataFlow flow, IMMDevice **dev_out)
{
    IMMDeviceEnumerator *e = NULL;
    IMMDevice *dev = NULL;
    IAudioClient *ac = NULL;
    HRESULT hr = CoCreateInstance(&CLSID_MMDeviceEnumerator, NULL, CLSCTX_ALL, &IID_IMMDeviceEnumerator, (void **)&e);
    if (FAILED(hr)) {
        fail("CoCreateInstance(MMDeviceEnumerator)", hr);
        return NULL;
    }
    hr = IMMDeviceEnumerator_GetDefaultAudioEndpoint(e, flow, eConsole, &dev);
    if (FAILED(hr)) {
        fail("GetDefaultAudioEndpoint", hr);
        return NULL;
    }
    hr = IMMDevice_Activate(dev, &IID_IAudioClient, CLSCTX_ALL, NULL, (void **)&ac);
    if (FAILED(hr)) {
        fail("IMMDevice::Activate", hr);
        return NULL;
    }
    *dev_out = dev;
    return ac;
}

static int wasapi(double hz, DWORD ms)
{
    IMMDevice *dev;
    IAudioClient *ac = open_client(eRender, &dev);
    IAudioRenderClient *rc = NULL;
    IAudioClock *clock = NULL;
    WAVEFORMATEX *f = NULL;
    UINT32 size, pad;
    UINT64 written = 0, total, pos = 0, freq = 1;
    DWORD start;
    HRESULT hr;
    if (!ac)
        return 1;
    if (FAILED(hr = IAudioClient_GetMixFormat(ac, &f)))
        return fail("GetMixFormat", hr);
    if (FAILED(hr = IAudioClient_Initialize(ac, AUDCLNT_SHAREMODE_SHARED, 0, 2000000, 0, f, NULL)))
        return fail("Initialize", hr);
    if (FAILED(hr = IAudioClient_GetBufferSize(ac, &size)))
        return fail("GetBufferSize", hr);
    if (FAILED(hr = IAudioClient_GetService(ac, &IID_IAudioRenderClient, (void **)&rc)))
        return fail("GetService(IAudioRenderClient)", hr);
    IAudioClient_GetService(ac, &IID_IAudioClock, (void **)&clock);
    total = (UINT64)f->nSamplesPerSec * ms / 1000;
    printf("sndtest: wasapi mix format %lu Hz, %u channels, %u bits%s, buffer %u frames\n", f->nSamplesPerSec, f->nChannels,
           f->wBitsPerSample, is_float(f) ? " float" : "", size);
    start = GetTickCount();
    while (written < total) {
        BYTE *buf;
        UINT32 n;
        if (FAILED(hr = IAudioClient_GetCurrentPadding(ac, &pad)))
            return fail("GetCurrentPadding", hr);
        n = size - pad;
        if (n > total - written)
            n = (UINT32)(total - written);
        if (n > 0) {
            if (FAILED(hr = IAudioRenderClient_GetBuffer(rc, n, &buf)))
                return fail("GetBuffer", hr);
            fill(buf, n, f, hz, written);
            IAudioRenderClient_ReleaseBuffer(rc, n, 0);
            if (written == 0 && FAILED(hr = IAudioClient_Start(ac)))
                return fail("Start", hr);
            written += n;
        }
        Sleep(10);
    }
    /* Until it played out. */
    while (SUCCEEDED(IAudioClient_GetCurrentPadding(ac, &pad)) && pad > 0 && GetTickCount() - start < ms + 5000)
        Sleep(10);
    if (clock) {
        IAudioClock_GetFrequency(clock, &freq);
        IAudioClock_GetPosition(clock, &pos, NULL);
    }
    IAudioClient_Stop(ac);
    printf("sndtest: wasapi played %llu frames of %.0f Hz in %lu ms, clock at %llu ms\n", (unsigned long long)written, hz,
           GetTickCount() - start, (unsigned long long)(pos * 1000 / (freq ? freq : 1)));
    return 0;
}

static int waveout(double hz, DWORD ms)
{
    WAVEFORMATEX f = {WAVE_FORMAT_PCM, 2, 48000, 48000 * 4, 4, 16, 0};
    HWAVEOUT wo;
    WAVEHDR h;
    DWORD frames = 48000 * ms / 1000, start;
    MMRESULT r = waveOutOpen(&wo, WAVE_MAPPER, &f, 0, 0, CALLBACK_NULL);
    if (r != MMSYSERR_NOERROR) {
        printf("sndtest: waveOutOpen failed: %u\n", r);
        return 1;
    }
    memset(&h, 0, sizeof(h));
    h.dwBufferLength = frames * 4;
    h.lpData = malloc(h.dwBufferLength);
    fill((BYTE *)h.lpData, frames, &f, hz, 0);
    waveOutPrepareHeader(wo, &h, sizeof(h));
    start = GetTickCount();
    waveOutWrite(wo, &h, sizeof(h));
    while (!(h.dwFlags & WHDR_DONE) && GetTickCount() - start < ms + 5000)
        Sleep(10);
    printf("sndtest: waveout played %lu frames of %.0f Hz in %lu ms%s\n", frames, hz, GetTickCount() - start,
           h.dwFlags & WHDR_DONE ? "" : " (not done)");
    waveOutUnprepareHeader(wo, &h, sizeof(h));
    waveOutClose(wo);
    return 0;
}

/* Little-endian words for the WAV header. */
static void put32(FILE *o, DWORD v) { fwrite(&v, 4, 1, o); }
static void put16(FILE *o, WORD v) { fwrite(&v, 2, 1, o); }

static int capture(DWORD ms, const char *path)
{
    IMMDevice *dev;
    IAudioClient *ac = open_client(eCapture, &dev);
    IAudioCaptureClient *cc = NULL;
    WAVEFORMATEX *f = NULL;
    UINT64 frames = 0, want;
    DWORD start;
    short *pcm;
    long crossings = 0, peak = 0;
    short last = 0;
    HRESULT hr;
    FILE *o;
    if (!ac)
        return 1;
    if (FAILED(hr = IAudioClient_GetMixFormat(ac, &f)))
        return fail("GetMixFormat", hr);
    if (FAILED(hr = IAudioClient_Initialize(ac, AUDCLNT_SHAREMODE_SHARED, 0, 2000000, 0, f, NULL)))
        return fail("Initialize", hr);
    if (FAILED(hr = IAudioClient_GetService(ac, &IID_IAudioCaptureClient, (void **)&cc)))
        return fail("GetService(IAudioCaptureClient)", hr);
    want = (UINT64)f->nSamplesPerSec * ms / 1000;
    pcm = malloc((size_t)want * 2 * sizeof(short) + 65536 * 8);
    printf("sndtest: capture mix format %lu Hz, %u channels, %u bits%s\n", f->nSamplesPerSec, f->nChannels, f->wBitsPerSample,
           is_float(f) ? " float" : "");
    if (FAILED(hr = IAudioClient_Start(ac)))
        return fail("Start", hr);
    start = GetTickCount();
    while (GetTickCount() - start < ms) {
        UINT32 n, i;
        BYTE *buf;
        DWORD flags;
        Sleep(10);
        while (SUCCEEDED(IAudioCaptureClient_GetNextPacketSize(cc, &n)) && n > 0) {
            if (FAILED(hr = IAudioCaptureClient_GetBuffer(cc, &buf, &n, &flags, NULL, NULL)))
                return fail("GetBuffer", hr);
            for (i = 0; i < n && frames < want + 65536; i++, frames++) {
                int c;
                for (c = 0; c < 2; c++) {
                    int ch = c < f->nChannels ? c : 0;
                    double v;
                    if (flags & AUDCLNT_BUFFERFLAGS_SILENT)
                        v = 0;
                    else if (is_float(f))
                        v = ((float *)buf)[i * f->nChannels + ch];
                    else
                        v = ((short *)buf)[i * f->nChannels + ch] / 32768.0;
                    pcm[frames * 2 + c] = (short)(v * 32767);
                }
                if (labs(pcm[frames * 2]) > peak)
                    peak = labs(pcm[frames * 2]);
                if (last < 0 && pcm[frames * 2] >= 0)
                    crossings++;
                last = pcm[frames * 2];
            }
            IAudioCaptureClient_ReleaseBuffer(cc, n);
        }
    }
    ms = GetTickCount() - start;
    IAudioClient_Stop(ac);
    printf("sndtest: capture took %llu frames in %lu ms, peak %ld, %ld Hz\n", (unsigned long long)frames, ms, peak,
           frames ? (long)(crossings * (double)f->nSamplesPerSec / frames) : 0);
    if (path && (o = fopen(path, "wb"))) {
        DWORD bytes = (DWORD)frames * 4;
        fwrite("RIFF", 1, 4, o);
        put32(o, 36 + bytes);
        fwrite("WAVEfmt ", 1, 8, o);
        put32(o, 16);
        put16(o, 1);
        put16(o, 2);
        put32(o, f->nSamplesPerSec);
        put32(o, f->nSamplesPerSec * 4);
        put16(o, 4);
        put16(o, 16);
        fwrite("data", 1, 4, o);
        put32(o, bytes);
        fwrite(pcm, 1, bytes, o);
        fclose(o);
    }
    return 0;
}

int main(int argc, char **argv)
{
    const char *mode = argc > 1 ? argv[1] : "wasapi";
    int r;
    setvbuf(stdout, NULL, _IONBF, 0);
    CoInitializeEx(NULL, COINIT_MULTITHREADED);
    if (!strcmp(mode, "waveout"))
        r = waveout(argc > 2 ? atof(argv[2]) : 880, argc > 3 ? atoi(argv[3]) : 2000);
    else if (!strcmp(mode, "capture"))
        r = capture(argc > 2 ? atoi(argv[2]) : 2000, argc > 3 ? argv[3] : NULL);
    else
        r = wasapi(argc > 2 ? atof(argv[2]) : 660, argc > 3 ? atoi(argv[3]) : 2000);
    printf("sndtest: %s done (%d)\n", mode, r);
    CoUninitialize();
    return r;
}
