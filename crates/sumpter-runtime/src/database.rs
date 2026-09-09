//! SeaORM boundary for the synchronous engine and its single write worker.
//!
//! All connections, transactions and SQL execution belong to SeaORM. The
//! dedicated executor avoids nested Tokio runtimes when Admin calls a sync
//! query. Raw analytics retain their SQL; streaming uses bounded delivery and
//! cancels the database cursor when an export consumer stops reading.

use futures_util::{StreamExt, future::BoxFuture, stream::BoxStream};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DatabaseTransaction, DbBackend,
    DbErr, ExecResult, QueryResult, Statement as OrmStatement, StreamTrait, TransactionTrait,
    TryGetable,
};
use std::{
    fmt,
    future::Future,
    marker::PhantomData,
    ops::Deref,
    path::Path,
    sync::{Arc, OnceLock},
    time::Duration,
};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Database(DbErr),
    QueryReturnedNoRows,
    InvalidQuery,
    InvalidParameterName(String),
    Conversion(Box<dyn std::error::Error + Send + Sync>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => error.fmt(f),
            // Existing Admin adapters use this message to preserve HTTP 404.
            Self::QueryReturnedNoRows => f.write_str("Query returned no rows"),
            Self::InvalidQuery => f.write_str("invalid query"),
            Self::InvalidParameterName(message) => f.write_str(message),
            Self::Conversion(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::Conversion(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<DbErr> for Error {
    fn from(error: DbErr) -> Self {
        Self::Database(error)
    }
}

pub trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}
impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn executor() -> Result<&'static tokio::runtime::Runtime> {
    static EXECUTOR: OnceLock<std::result::Result<tokio::runtime::Runtime, String>> =
        OnceLock::new();
    EXECUTOR
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("runtime-orm")
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| Error::InvalidParameterName(error.clone()))
}

fn run<T: Send + 'static>(
    future: impl Future<Output = std::result::Result<T, DbErr>> + Send + 'static,
) -> Result<T> {
    // These futures are polled synchronously outside the caller's Tokio
    // scheduler. Its cooperative budget cannot be replenished until we
    // return, so inherited budget exhaustion would otherwise deadlock.
    futures_executor::block_on(tokio::task::unconstrained(executor()?.spawn(future)))
        .map_err(|error| Error::InvalidParameterName(format!("ORM executor: {error}")))?
        .map_err(Error::from)
}

pub(crate) enum Session {
    Connection(DatabaseConnection),
    Transaction(DatabaseTransaction),
}

#[async_trait::async_trait]
impl ConnectionTrait for Session {
    fn get_database_backend(&self) -> DbBackend {
        DbBackend::Sqlite
    }
    async fn execute(&self, statement: OrmStatement) -> std::result::Result<ExecResult, DbErr> {
        match self {
            Self::Connection(db) => db.execute(statement).await,
            Self::Transaction(db) => db.execute(statement).await,
        }
    }
    async fn execute_unprepared(&self, sql: &str) -> std::result::Result<ExecResult, DbErr> {
        match self {
            Self::Connection(db) => db.execute_unprepared(sql).await,
            Self::Transaction(db) => db.execute_unprepared(sql).await,
        }
    }
    async fn query_one(
        &self,
        statement: OrmStatement,
    ) -> std::result::Result<Option<QueryResult>, DbErr> {
        match self {
            Self::Connection(db) => db.query_one(statement).await,
            Self::Transaction(db) => db.query_one(statement).await,
        }
    }
    async fn query_all(
        &self,
        statement: OrmStatement,
    ) -> std::result::Result<Vec<QueryResult>, DbErr> {
        match self {
            Self::Connection(db) => db.query_all(statement).await,
            Self::Transaction(db) => db.query_all(statement).await,
        }
    }
}

impl StreamTrait for Session {
    type Stream<'a> = BoxStream<'a, std::result::Result<QueryResult, DbErr>>;
    fn stream<'a>(
        &'a self,
        statement: OrmStatement,
    ) -> BoxFuture<'a, std::result::Result<Self::Stream<'a>, DbErr>> {
        Box::pin(async move {
            let stream: Self::Stream<'a> = match self {
                Self::Connection(db) => Box::pin(db.stream(statement).await?),
                Self::Transaction(db) => Box::pin(db.stream(statement).await?),
            };
            Ok(stream)
        })
    }
}

/// One physical SQLite connection. Each read transaction keeps its snapshot;
/// the writer never competes with another writer inside an implicit pool.
pub struct Connection {
    session: Option<Arc<Session>>,
}

#[derive(Clone, Copy)]
pub enum OpenFlags {
    ReadOnly,
}
impl OpenFlags {
    pub const SQLITE_OPEN_READ_ONLY: Self = Self::ReadOnly;
}

impl Connection {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::connect(path.as_ref(), false, false)
    }
    pub fn open_with_flags(path: impl AsRef<Path>, _flags: OpenFlags) -> Result<Self> {
        Self::connect(path.as_ref(), true, false)
    }
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::connect(Path::new(":memory:"), false, true)
    }

    fn connect(path: &Path, read_only: bool, in_memory: bool) -> Result<Self> {
        let path = path.to_owned();
        let mut options = ConnectOptions::new("sqlite://runtime");
        options
            .max_connections(1)
            .min_connections(1)
            .sqlx_logging(false)
            .connect_timeout(Duration::from_secs(5))
            .acquire_timeout(Duration::from_secs(5));
        options.map_sqlx_sqlite_opts(move |options| {
            options
                .filename(&path)
                .in_memory(in_memory)
                .shared_cache(false)
                .read_only(read_only)
                .create_if_missing(!read_only)
                .busy_timeout(Duration::from_secs(5))
                .pragma("foreign_keys", "ON")
                .pragma("cache_size", "-2048")
        });
        let db = run(async move { Database::connect(options).await })?;
        Ok(Self {
            session: Some(Arc::new(Session::Connection(db))),
        })
    }

    pub(crate) fn orm<T, F, Fut>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Arc<Session>) -> Fut,
        Fut: Future<Output = std::result::Result<T, DbErr>> + Send + 'static,
    {
        run(operation(
            self.session.as_ref().expect("open ORM connection").clone(),
        ))
    }

    pub fn busy_timeout(&self, duration: Duration) -> Result<()> {
        self.execute_batch(&format!(
            "PRAGMA busy_timeout = {}",
            duration.as_millis().min(i32::MAX as u128)
        ))
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let sql = sql.to_owned();
        self.orm(move |db| async move { db.execute_unprepared(&sql).await.map(|_| ()) })
    }

    pub fn execute(&self, sql: &str, params: impl Params) -> Result<usize> {
        let statement = statement(sql, params);
        self.orm(move |db| async move {
            db.execute(statement)
                .await
                .map(|result| result.rows_affected() as usize)
        })
    }

    pub fn query_row<T>(
        &self,
        sql: &str,
        params: impl Params,
        map: impl FnOnce(&Row) -> Result<T>,
    ) -> Result<T> {
        let statement = statement(sql, params);
        let row = self
            .orm(move |db| async move { db.query_one(statement).await })?
            .ok_or(Error::QueryReturnedNoRows)?;
        map(&Row(row))
    }

    pub fn prepare<'a>(&'a self, sql: &str) -> Result<Statement<'a>> {
        Ok(Statement {
            connection: self,
            sql: sql.to_owned(),
        })
    }

    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        self.transaction_with_behavior(TransactionBehavior::Deferred)
    }

    pub fn transaction_with_behavior(&mut self, _: TransactionBehavior) -> Result<Transaction<'_>> {
        let transaction = self.orm(|db| async move {
            match db.as_ref() {
                Session::Connection(db) => db.begin().await,
                Session::Transaction(_) => Err(DbErr::Custom("nested runtime transaction".into())),
            }
        })?;
        Ok(Transaction {
            connection: Connection {
                session: Some(Arc::new(Session::Transaction(transaction))),
            },
            parent: PhantomData,
        })
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(session) = self
            .session
            .take()
            .and_then(|session| Arc::try_unwrap(session).ok())
        {
            let _ = run(async move {
                match session {
                    Session::Connection(db) => db.close().await,
                    Session::Transaction(db) => db.rollback().await,
                }
            });
        }
    }
}

#[derive(Clone, Copy)]
pub enum TransactionBehavior {
    Deferred,
}
pub struct Transaction<'a> {
    connection: Connection,
    parent: PhantomData<&'a mut Connection>,
}
impl Deref for Transaction<'_> {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}
impl Transaction<'_> {
    pub fn commit(mut self) -> Result<()> {
        let session = self.connection.session.take().expect("open transaction");
        let session = Arc::try_unwrap(session).map_err(|_| {
            Error::InvalidParameterName("transaction still has an active cursor".into())
        })?;
        run(async move {
            match session {
                Session::Transaction(db) => db.commit().await,
                Session::Connection(_) => unreachable!("transaction session"),
            }
        })
    }
}

pub struct Row(QueryResult);
impl Row {
    pub fn get<I: sea_orm::ColIdx, T: TryGetable>(&self, index: I) -> Result<T> {
        self.0.try_get_by(index).map_err(Error::from)
    }

    pub fn model<T: sea_orm::FromQueryResult>(&self) -> Result<T> {
        T::from_query_result(&self.0, "").map_err(Error::from)
    }
}

pub struct Statement<'a> {
    connection: &'a Connection,
    sql: String,
}
impl Statement<'_> {
    pub fn query(&mut self, params: impl Params) -> Result<Rows> {
        let statement = statement(&self.sql, params);
        let session = self
            .connection
            .session
            .as_ref()
            .expect("open connection")
            .clone();
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        let task = executor()?.spawn(async move {
            let result = async {
                let mut stream = session.stream(statement).await?;
                loop {
                    tokio::select! {
                        _ = sender.closed() => break,
                        row = stream.next() => match row {
                            Some(Ok(row)) => if sender.send(Ok(Row(row))).await.is_err() { break; },
                            Some(Err(error)) => return Err(error),
                            None => break,
                        }
                    }
                }
                Ok::<_, DbErr>(())
            }
            .await;
            if let Err(error) = result {
                let _ = sender.send(Err(Error::from(error))).await;
            }
            drop(session);
        });
        Ok(Rows {
            receiver,
            task: Some(task),
            current: None,
        })
    }
    pub fn query_map<T, F: FnMut(&Row) -> Result<T>>(
        &mut self,
        params: impl Params,
        map: F,
    ) -> Result<MappedRows<F>> {
        Ok(MappedRows {
            rows: self.query(params)?,
            map,
        })
    }
}

pub struct Rows {
    receiver: tokio::sync::mpsc::Receiver<Result<Row>>,
    task: Option<tokio::task::JoinHandle<()>>,
    current: Option<Row>,
}
impl Rows {
    pub fn next(&mut self) -> Result<Option<&Row>> {
        self.current = futures_executor::block_on(tokio::task::unconstrained(self.receiver.recv()))
            .transpose()?;
        if self.current.is_none() {
            if let Some(task) = self.task.take() {
                futures_executor::block_on(tokio::task::unconstrained(task))
                    .map_err(|error| Error::InvalidParameterName(format!("ORM stream: {error}")))?;
            }
        }
        Ok(self.current.as_ref())
    }
}
impl Drop for Rows {
    fn drop(&mut self) {
        self.receiver.close();
        if let Some(task) = self.task.take() {
            task.abort();
            // Release the cursor and transaction lock before the caller proceeds.
            let _ = futures_executor::block_on(tokio::task::unconstrained(task));
        }
    }
}
pub struct MappedRows<F> {
    rows: Rows,
    map: F,
}
impl<T, F: FnMut(&Row) -> Result<T>> Iterator for MappedRows<F> {
    type Item = Result<T>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.rows.next() {
            Ok(Some(row)) => Some((self.map)(row)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        }
    }
}

pub trait BindValue {
    fn bind_value(&self) -> sea_orm::Value;
}
macro_rules! bind_value {
    ($($ty:ty),*) => { $(impl BindValue for $ty { fn bind_value(&self) -> sea_orm::Value { (*self).into() } })* };
}
bind_value!(bool, i8, i16, i32, i64, u8, u16, u32, f32, f64);
impl BindValue for str {
    fn bind_value(&self) -> sea_orm::Value {
        self.to_owned().into()
    }
}
impl BindValue for String {
    fn bind_value(&self) -> sea_orm::Value {
        self.clone().into()
    }
}
impl<T: BindValue + ?Sized> BindValue for &T {
    fn bind_value(&self) -> sea_orm::Value {
        (*self).bind_value()
    }
}
impl<T: BindValue> BindValue for Option<T> {
    fn bind_value(&self) -> sea_orm::Value {
        self.as_ref()
            .map_or(sea_orm::Value::String(None), BindValue::bind_value)
    }
}
impl BindValue for sea_orm::Value {
    fn bind_value(&self) -> sea_orm::Value {
        self.clone()
    }
}

pub trait Params {
    fn values(self) -> Vec<sea_orm::Value>;
}
impl<T: BindValue, const N: usize> Params for [T; N] {
    fn values(self) -> Vec<sea_orm::Value> {
        self.iter().map(BindValue::bind_value).collect()
    }
}
impl Params for Vec<sea_orm::Value> {
    fn values(self) -> Vec<sea_orm::Value> {
        self
    }
}
macro_rules! tuple_params {
    ($($name:ident),+) => {
        impl<$($name: BindValue),+> Params for ($($name,)+) {
            #[allow(non_snake_case)]
            fn values(self) -> Vec<sea_orm::Value> { let ($($name,)+) = self; vec![$($name.bind_value()),+] }
        }
    }
}
tuple_params!(A);
tuple_params!(A, B);
tuple_params!(A, B, C);
tuple_params!(A, B, C, D);
tuple_params!(A, B, C, D, E);
tuple_params!(A, B, C, D, E, F);

macro_rules! params {
    ($($value:expr),* $(,)?) => { vec![$($crate::database::BindValue::bind_value(&$value)),*] as Vec<sea_orm::Value> };
}
pub(crate) use params;
pub fn params_from_iter<T: BindValue>(values: impl IntoIterator<Item = T>) -> Vec<sea_orm::Value> {
    values.into_iter().map(|value| value.bind_value()).collect()
}

pub mod types {
    #[derive(Debug, Clone)]
    pub enum Value {
        Integer(i64),
        Real(f64),
        Text(String),
    }
}
impl BindValue for types::Value {
    fn bind_value(&self) -> sea_orm::Value {
        match self {
            Self::Integer(value) => (*value).into(),
            Self::Real(value) => (*value).into(),
            Self::Text(value) => value.clone().into(),
        }
    }
}

fn statement(sql: &str, params: impl Params) -> OrmStatement {
    OrmStatement::from_sql_and_values(DbBackend::Sqlite, sql, params.values())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::runtime_meta;
    use sea_orm::{EntityTrait, Set};

    #[tokio::test(flavor = "current_thread")]
    async fn orm_failure_rolls_back_inside_a_sync_api_called_from_tokio() {
        let mut connection = Connection::open_in_memory().unwrap();
        crate::entities::create_schema(&mut connection).unwrap();
        {
            let transaction = connection.transaction().unwrap();
            let result = transaction.orm(|db| async move {
                let model = runtime_meta::ActiveModel {
                    key: Set("atomic".into()),
                    value: Set("first".into()),
                };
                runtime_meta::Entity::insert(model.clone())
                    .exec_without_returning(db.as_ref())
                    .await?;
                runtime_meta::Entity::insert(model)
                    .exec_without_returning(db.as_ref())
                    .await?;
                Ok(())
            });
            assert!(
                result.is_err(),
                "duplicate key must fail after the first insert"
            );
        }
        let count = connection
            .query_row("SELECT COUNT(*) FROM runtime_meta", params![], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(
            count, 0,
            "dropping a failed transaction must roll back its earlier writes"
        );
    }

    #[test]
    fn synchronous_calls_and_streams_do_not_exhaust_the_callers_tokio_budget() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let connection = Connection::open_in_memory().unwrap();
                // Engine startup performs many synchronous database calls
                // before returning control to its current-thread runtime.
                for _ in 0..256 {
                    assert_eq!(connection.query_row("SELECT 1", params![], |row| row.get::<_, i64>(0)).unwrap(), 1);
                }
                let mut statement = connection.prepare("WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<1024) SELECT n FROM numbers").unwrap();
                let rows = statement.query_map(params![], |row| row.get::<_, i64>(0)).unwrap().collect::<Result<Vec<_>>>().unwrap();
                assert_eq!(rows.len(), 1024);
            });
            sender.send(()).unwrap();
        });
        receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("sync ORM bridge must complete without yielding the caller's Tokio task");
        thread.join().unwrap();
    }

    #[test]
    fn dropping_a_large_stream_releases_the_transaction_cursor() {
        let mut connection = Connection::open_in_memory().unwrap();
        let transaction = connection.transaction().unwrap();
        {
            let mut statement = transaction.prepare("WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<1000000) SELECT n FROM numbers").unwrap();
            let mut rows = statement.query(params![]).unwrap();
            assert_eq!(rows.next().unwrap().unwrap().get::<_, i64>(0).unwrap(), 1);
            // Cancel before consuming a million rows. Drop must release both
            // the bounded channel and SeaORM's transaction stream lock.
        }
        assert_eq!(
            transaction
                .query_row("SELECT 42", params![], |row| row.get::<_, i64>(0))
                .unwrap(),
            42
        );
        transaction.commit().unwrap();
    }

    #[test]
    fn readonly_connections_reject_writes_and_never_create_missing_files() {
        let root = std::env::temp_dir().join(format!(
            "sumpter-orm-readonly-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("runtime.sqlite3");
        let missing = root.join("missing.sqlite3");
        assert!(Connection::open_with_flags(&missing, OpenFlags::SQLITE_OPEN_READ_ONLY).is_err());
        assert!(!missing.exists());
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE test(value INTEGER NOT NULL); INSERT INTO test VALUES(7)")
            .unwrap();
        drop(connection);
        let before = std::fs::read(&path).unwrap();
        let reader = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            reader
                .query_row("SELECT value FROM test", params![], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            7
        );
        assert!(
            reader
                .execute("INSERT INTO test VALUES(8)", params![])
                .is_err()
        );
        drop(reader);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn entity_schema_keeps_singletons_price_constraints_and_event_nulls() {
        let mut connection = Connection::open_in_memory().unwrap();
        crate::entities::create_schema(&mut connection).unwrap();
        assert!(
            connection
                .execute(
                    "INSERT INTO runtime_retention(id,revision,updated_at) VALUES(2,1,0)",
                    params![]
                )
                .is_err()
        );
        assert!(connection.execute("INSERT INTO runtime_model_prices(model_key,effective_from,input_per_million_micros,created_at,updated_at) VALUES('test',1,-1,0,0)", params![]).is_err());
        assert!(connection.execute("INSERT INTO runtime_model_prices(model_key,effective_from,effective_to,created_at,updated_at) VALUES('test',2,1,0,0)", params![]).is_err());
        connection.execute("INSERT INTO runtime_events(seq,change_seq,event_id,timestamp,kind,status_code,is_in_flight,payload_json,created_at,updated_at) VALUES(1,1,'nullable',0,'client',200,0,'{}',0,0)", params![]).unwrap();
        let row = connection
            .orm(|db| async move {
                crate::entities::runtime_events::Entity::find_by_id(1_i64)
                    .one(db.as_ref())
                    .await
            })
            .unwrap()
            .unwrap();
        assert_eq!(row.projection_version, 0);
        assert_eq!(row.payload_bytes, 0);
        assert_eq!(row.input_tokens, None);
        assert_eq!(row.agent_role, None);
        assert!(connection.execute("INSERT INTO runtime_events(seq,change_seq,event_id,timestamp,kind,status_code,is_in_flight,payload_json,created_at,updated_at) VALUES(2,2,'nullable',0,'client',200,0,'{}',0,0)", params![]).is_err());
    }
}
