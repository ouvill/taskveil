use crate::*;

pub struct SqliteTimerSessionRepository {
    connection: Connection,
}

impl SqliteTimerSessionRepository {
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

impl TimerSessionRepository for SqliteTimerSessionRepository {
    fn load_active(&self) -> Result<Option<ActiveTimerSession>, StorageError> {
        self.connection
            .query_row(
                "SELECT session_id, task_id, mode, phase, state, started_at,
                        last_resumed_at, accumulated_active_ms, target_duration_ms
                 FROM active_timer_session WHERE singleton = 1",
                [],
                row_to_active_timer_session,
            )
            .optional()?
            .transpose()
    }

    fn start_active(
        &mut self,
        session: ActiveTimerSession,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        start_active_timer_session_on(&self.connection, session, updated_at)
    }

    fn update_active(
        &mut self,
        session: ActiveTimerSession,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        update_active_timer_session_on(&self.connection, session, updated_at)
    }

    fn clear_active(&mut self, expected_session_id: Uuid) -> Result<bool, StorageError> {
        Ok(self.connection.execute(
            "DELETE FROM active_timer_session WHERE singleton = 1 AND session_id = ?1",
            [expected_session_id.to_string()],
        )? == 1)
    }

    fn clear_active_for_task(&mut self, task_id: Uuid) -> Result<bool, StorageError> {
        Ok(self.connection.execute(
            "DELETE FROM active_timer_session WHERE task_id = ?1",
            [task_id.to_string()],
        )? == 1)
    }

    fn get_completed(&self, id: Uuid) -> Result<CompletedTimerSession, StorageError> {
        get_completed_timer_session_on(&self.connection, id)
    }

    fn insert_completed(&mut self, session: CompletedTimerSession) -> Result<bool, StorageError> {
        insert_completed_timer_session_on(&self.connection, session)
    }

    fn list_completed(&self) -> Result<Vec<CompletedTimerSession>, StorageError> {
        list_completed_timer_sessions_on(
            &self.connection,
            "SELECT id, task_id, mode, finish_kind, started_at, ended_at,
                    active_duration_ms, created_at
             FROM timer_sessions ORDER BY started_at, id",
            [],
        )
    }

    fn list_completed_by_task(
        &self,
        task_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, StorageError> {
        list_completed_timer_sessions_on(
            &self.connection,
            "SELECT id, task_id, mode, finish_kind, started_at, ended_at,
                    active_duration_ms, created_at
             FROM timer_sessions WHERE task_id = ?1 ORDER BY started_at, id",
            [task_id.to_string()],
        )
    }

    fn list_completed_by_list(
        &self,
        list_id: Uuid,
    ) -> Result<Vec<CompletedTimerSession>, StorageError> {
        list_completed_timer_sessions_on(
            &self.connection,
            "SELECT timer.id, timer.task_id, timer.mode, timer.finish_kind,
                    timer.started_at, timer.ended_at, timer.active_duration_ms, timer.created_at
             FROM timer_sessions timer
             INNER JOIN tasks ON tasks.id = timer.task_id
             WHERE tasks.list_id = ?1 ORDER BY timer.started_at, timer.id",
            [list_id.to_string()],
        )
    }

    fn delete_completed(&mut self, id: Uuid) -> Result<bool, StorageError> {
        Ok(self
            .connection
            .execute("DELETE FROM timer_sessions WHERE id = ?1", [id.to_string()])?
            == 1)
    }
}

pub(super) fn start_active_timer_session_on(
    connection: &Connection,
    session: ActiveTimerSession,
    updated_at: i64,
) -> Result<(), StorageError> {
    validate_active_timer_session(&session).map_err(StorageError::InvalidActiveTimerUpdate)?;
    if let Some(task_id) = session.task_id {
        require_existing_timer_task(connection, task_id)?;
    }
    let changed = connection.execute(
        "INSERT INTO active_timer_session (
             singleton, session_id, task_id, mode, phase, state, started_at,
             last_resumed_at, accumulated_active_ms, target_duration_ms, updated_at
         ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(singleton) DO NOTHING",
        params![
            session.session_id.to_string(),
            session.task_id.map(|id| id.to_string()),
            timer_mode_str(session.mode),
            timer_phase_str(session.phase),
            timer_run_state_str(session.state),
            session.started_at,
            session.last_resumed_at,
            session.accumulated_active_ms,
            session.target_duration_ms,
            updated_at,
        ],
    )?;
    if changed == 0 {
        let existing = load_active_timer_session_on(connection)?.ok_or_else(|| {
            StorageError::IncompatibleSchema("active timer conflict without row".to_string())
        })?;
        return Err(StorageError::ActiveTimerConflict(existing.session_id));
    }
    Ok(())
}

pub(super) fn update_active_timer_session_on(
    connection: &Connection,
    session: ActiveTimerSession,
    updated_at: i64,
) -> Result<(), StorageError> {
    let current = load_active_timer_session_on(connection)?
        .ok_or(StorageError::NotFound(session.session_id))?;
    validate_active_timer_update(&current, &session)
        .map_err(StorageError::InvalidActiveTimerUpdate)?;
    let changed = connection.execute(
        "UPDATE active_timer_session
         SET state = ?1, last_resumed_at = ?2, accumulated_active_ms = ?3,
             target_duration_ms = ?4, updated_at = ?5
         WHERE singleton = 1 AND session_id = ?6",
        params![
            timer_run_state_str(session.state),
            session.last_resumed_at,
            session.accumulated_active_ms,
            session.target_duration_ms,
            updated_at,
            session.session_id.to_string(),
        ],
    )?;
    if changed != 1 {
        return Err(StorageError::NotFound(session.session_id));
    }
    Ok(())
}

fn load_active_timer_session_on(
    connection: &Connection,
) -> Result<Option<ActiveTimerSession>, StorageError> {
    connection
        .query_row(
            "SELECT session_id, task_id, mode, phase, state, started_at,
                    last_resumed_at, accumulated_active_ms, target_duration_ms
             FROM active_timer_session WHERE singleton = 1",
            [],
            row_to_active_timer_session,
        )
        .optional()?
        .transpose()
}

pub(super) fn list_completed_timer_sessions_on<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<CompletedTimerSession>, StorageError> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map(params, row_to_completed_timer_session)?;
    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row??);
    }
    Ok(sessions)
}

pub(super) fn get_completed_timer_session_on(
    connection: &Connection,
    id: Uuid,
) -> Result<CompletedTimerSession, StorageError> {
    connection
        .query_row(
            "SELECT id, task_id, mode, finish_kind, started_at, ended_at,
                    active_duration_ms, created_at
             FROM timer_sessions WHERE id = ?1",
            [id.to_string()],
            row_to_completed_timer_session,
        )
        .optional()?
        .transpose()?
        .ok_or(StorageError::NotFound(id))
}

pub(super) fn insert_completed_timer_session_on(
    connection: &Connection,
    session: CompletedTimerSession,
) -> Result<bool, StorageError> {
    validate_completed_timer_session(&session)
        .map_err(|error| StorageError::IncompatibleSchema(error.to_string()))?;
    require_existing_timer_task(connection, session.task_id)?;
    match get_completed_timer_session_on(connection, session.id) {
        Ok(existing) if existing == session => return Ok(false),
        Ok(_) => {
            return Err(StorageError::IncompatibleSchema(
                "immutable timer session contents conflict".to_string(),
            ))
        }
        Err(StorageError::NotFound(_)) => {}
        Err(error) => return Err(error),
    }
    connection.execute(
        "INSERT INTO timer_sessions (
             id, task_id, mode, finish_kind, started_at, ended_at,
             active_duration_ms, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            session.id.to_string(),
            session.task_id.to_string(),
            timer_mode_str(session.mode),
            timer_finish_kind_str(session.finish_kind),
            session.started_at,
            session.ended_at,
            session.active_duration_ms,
            session.created_at,
        ],
    )?;
    Ok(true)
}

pub(super) fn row_to_active_timer_session(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<ActiveTimerSession, StorageError>> {
    Ok((|| {
        let session = ActiveTimerSession {
            session_id: parse_uuid_column(row.get(0)?)?,
            task_id: row
                .get::<_, Option<String>>(1)?
                .map(parse_uuid_column)
                .transpose()?,
            mode: parse_timer_mode(&row.get::<_, String>(2)?)?,
            phase: parse_timer_phase(&row.get::<_, String>(3)?)?,
            state: parse_timer_run_state(&row.get::<_, String>(4)?)?,
            started_at: row.get(5)?,
            last_resumed_at: row.get(6)?,
            accumulated_active_ms: row.get(7)?,
            target_duration_ms: row.get(8)?,
        };
        validate_active_timer_session(&session)
            .map_err(|error| StorageError::IncompatibleSchema(error.to_string()))?;
        Ok(session)
    })())
}

fn row_to_completed_timer_session(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<CompletedTimerSession, StorageError>> {
    Ok((|| {
        let session = CompletedTimerSession {
            id: parse_uuid_column(row.get(0)?)?,
            task_id: parse_uuid_column(row.get(1)?)?,
            mode: parse_timer_mode(&row.get::<_, String>(2)?)?,
            finish_kind: parse_timer_finish_kind(&row.get::<_, String>(3)?)?,
            started_at: row.get(4)?,
            ended_at: row.get(5)?,
            active_duration_ms: row.get(6)?,
            created_at: row.get(7)?,
        };
        validate_completed_timer_session(&session)
            .map_err(|error| StorageError::IncompatibleSchema(error.to_string()))?;
        Ok(session)
    })())
}

fn parse_uuid_column(value: String) -> Result<Uuid, StorageError> {
    value.parse().map_err(StorageError::from)
}

fn require_existing_timer_task(connection: &Connection, task_id: Uuid) -> Result<(), StorageError> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?1)",
        [task_id.to_string()],
        |row| row.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(StorageError::NotFound(task_id))
    }
}

fn timer_mode_str(value: TimerMode) -> &'static str {
    match value {
        TimerMode::Pomodoro => "pomodoro",
        TimerMode::Stopwatch => "stopwatch",
    }
}

fn parse_timer_mode(value: &str) -> Result<TimerMode, StorageError> {
    match value {
        "pomodoro" => Ok(TimerMode::Pomodoro),
        "stopwatch" => Ok(TimerMode::Stopwatch),
        _ => Err(StorageError::IncompatibleSchema(
            "invalid timer mode".to_string(),
        )),
    }
}

fn timer_phase_str(value: TimerPhase) -> &'static str {
    match value {
        TimerPhase::Work => "work",
        TimerPhase::ShortBreak => "short_break",
        TimerPhase::LongBreak => "long_break",
    }
}

fn parse_timer_phase(value: &str) -> Result<TimerPhase, StorageError> {
    match value {
        "work" => Ok(TimerPhase::Work),
        "short_break" => Ok(TimerPhase::ShortBreak),
        "long_break" => Ok(TimerPhase::LongBreak),
        _ => Err(StorageError::IncompatibleSchema(
            "invalid timer phase".to_string(),
        )),
    }
}

fn timer_run_state_str(value: TimerRunState) -> &'static str {
    match value {
        TimerRunState::Running => "running",
        TimerRunState::Paused => "paused",
    }
}

fn parse_timer_run_state(value: &str) -> Result<TimerRunState, StorageError> {
    match value {
        "running" => Ok(TimerRunState::Running),
        "paused" => Ok(TimerRunState::Paused),
        _ => Err(StorageError::IncompatibleSchema(
            "invalid timer run state".to_string(),
        )),
    }
}

fn timer_finish_kind_str(value: TimerFinishKind) -> &'static str {
    match value {
        TimerFinishKind::Completed => "completed",
        TimerFinishKind::Interrupted => "interrupted",
    }
}

fn parse_timer_finish_kind(value: &str) -> Result<TimerFinishKind, StorageError> {
    match value {
        "completed" => Ok(TimerFinishKind::Completed),
        "interrupted" => Ok(TimerFinishKind::Interrupted),
        _ => Err(StorageError::IncompatibleSchema(
            "invalid timer finish kind".to_string(),
        )),
    }
}
