CREATE TABLE app_settings (
    key TEXT PRIMARY KEY NOT NULL CHECK (
        key IN (
            'ui_mode',
            'onboarding_completed',
            'calendar_week_start',
            'timer_settings_v1'
        )
    ),
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE internal_metadata (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

INSERT INTO app_settings (key, value, updated_at)
SELECT key, value, updated_at
FROM settings
WHERE key IN (
    'ui_mode',
    'onboarding_completed',
    'calendar_week_start',
    'timer_settings_v1'
);

INSERT INTO internal_metadata (key, value, updated_at)
SELECT key, value, updated_at
FROM settings
WHERE key NOT IN (
    'ui_mode',
    'onboarding_completed',
    'calendar_week_start',
    'timer_settings_v1'
);

DROP TABLE settings;
