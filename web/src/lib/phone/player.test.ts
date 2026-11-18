// Watching a camera (#61): what a screen offers, and what POP's answers mean.
import { describe, expect, it } from 'vitest';
import { addresses, catchUp, codecs, said } from './player';

describe('the camera player', () => {
  it('offers the codecs the browser can play, as POP takes them', () => {
    const chrome = (m: string) => m.includes('avc1') || m.includes('mp4a') || m.includes('opus');
    const offered = codecs(chrome);
    expect(offered).toBe('avc1.640029,avc1.64002A,avc1.640033,mp4a.40.2,mp4a.40.5,opus');
    // As pop-server's codec check has it: letters, digits, dots, commas.
    expect(offered).toMatch(/^[A-Za-z0-9.,]+$/);
  });

  it('offers nothing when the browser plays no video', () => {
    expect(codecs((m) => m.startsWith('audio/'))).toBe('');
    expect(codecs(() => false)).toBe('');
  });

  it('reads what POP says', () => {
    expect(said('{"type":"mse","value":"video/mp4; codecs=\\"avc1.420029\\""}')).toEqual({
      kind: 'playing',
      mime: 'video/mp4; codecs="avc1.420029"',
    });
    expect(said('{"type":"error","value":"unsupported"}')).toEqual({ kind: 'mjpeg' });
    expect(said('{"type":"error","value":"no_picture"}')).toEqual({ kind: 'no_picture' });
    expect(said('{"type":"error","value":"bad_request"}')).toEqual({ kind: 'no_picture' });
    expect(said('not json')).toEqual({ kind: 'unreadable' });
    expect(said('{"type":"mse","value":""}')).toEqual({ kind: 'unreadable' });
  });

  it('finds the camera on the page’s own POP, over wss when the page is https', () => {
    expect(addresses('p 1', { protocol: 'https:', host: 'pop.example.org' })).toEqual({
      mse: 'wss://pop.example.org/api/printers/p%201/camera/mse',
      mjpeg: '/api/printers/p%201/camera/mjpeg',
    });
    expect(addresses('p1', { protocol: 'http:', host: '10.0.0.5:8080' }).mse).toBe(
      'ws://10.0.0.5:8080/api/printers/p1/camera/mse',
    );
  });

  it('jumps forward only when it has fallen too far behind', () => {
    expect(catchUp(10, 12)).toBeNull();
    expect(catchUp(10, 20)).toBe(19.5);
  });
});
