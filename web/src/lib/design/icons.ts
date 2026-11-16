// Round 6's icons (#48), as kit.py draws them: square caps, round joins,
// on a 24-unit grid.
export const icons = {
  back: 'M15 5l-7 7 7 7',
  sprout: 'M12 21V11M12 14c-5 0-7-3-7-7 5 0 7 3 7 7zM12 11c0-5 3-8 8-8 0 4-3 8-8 8z',
  check: 'M5 12l5 5 9-10',
  lock: 'M6 11h12v9H6zM9 11V8a3 3 0 016 0v3',
  warn: 'M12 4l9 16H3zM12 10v4M12 17v.5',
  camera: 'M3 7h4l2-3h6l2 3h4v12H3zM12 10a3 3 0 100 6 3 3 0 000-6z',
  bell: 'M6 16V11a6 6 0 0112 0v5l2 2H4zM10 20h4',
  book: 'M4 5h6a2 2 0 012 2v12a2 2 0 00-2-2H4zM20 5h-6a2 2 0 00-2 2v12a2 2 0 012-2h6z',
  spool: 'M5 4h14v4H5zM5 16h14v4H5zM8 8h8v8H8z',
  plus: 'M12 5v14M5 12h14',
  upload: 'M12 19V6M6 11l6-6 6 6M5 21h14',
} as const;

export type IconName = keyof typeof icons;
