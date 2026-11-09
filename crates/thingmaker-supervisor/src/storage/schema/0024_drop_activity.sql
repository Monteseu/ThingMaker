-- Chill mode was removed, and with it the file activity it was drawn from.
DROP INDEX IF EXISTS activity_files_recent;
DROP TABLE IF EXISTS activity_files;
