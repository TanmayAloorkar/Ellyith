-- New account gate quarantines get their own reason.
ALTER TABLE quarantines DROP CONSTRAINT quarantines_reason_check;
ALTER TABLE quarantines ADD CONSTRAINT quarantines_reason_check CHECK (reason IN ('verification','manual','raid','alt','gate'));
