"""Trace the SQL inside a SQLAlchemy Session joined to a SQLite transaction."""

import platform
import sqlite3
from contextlib import closing
from pathlib import Path
from tempfile import TemporaryDirectory

import sqlalchemy
from sqlalchemy import URL, create_engine, event, text
from sqlalchemy.orm import Session


CREATE = "CREATE TABLE entries (id INTEGER PRIMARY KEY, label TEXT NOT NULL)"
SEED = "INSERT INTO entries (id, label) VALUES (1, 'baseline')"
INSERT = "INSERT INTO entries (id, label) VALUES (2, 'session')"
READ = "SELECT id, label FROM entries ORDER BY id"


def run_case(path: Path, *, explicit_begin: bool) -> list[tuple[int, str]]:
    legacy = sqlite3.LEGACY_TRANSACTION_CONTROL
    with closing(sqlite3.connect(path, autocommit=legacy)) as setup, setup:
        setup.execute(CREATE)
        setup.execute(SEED)

    engine = create_engine(
        URL.create("sqlite+pysqlite", database=str(path)),
        connect_args={"autocommit": legacy, "isolation_level": None if explicit_begin else ""},
    )
    if explicit_begin:
        @event.listens_for(engine, "begin")
        def begin(connection):
            connection.exec_driver_sql("BEGIN")

    try:
        with engine.connect() as connection:
            driver = connection.connection.driver_connection
            if driver.autocommit != legacy:
                raise RuntimeError(f"Expected legacy transaction control, got {driver.autocommit!r}")

            print(f"\n[{path.stem}]")
            print(f"sqlite3.autocommit=LEGACY_TRANSACTION_CONTROL, isolation_level={driver.isolation_level!r}")
            statements = []
            driver.set_trace_callback(statements.append)

            def checkpoint(step: str) -> None:
                print(
                    f"{step} | SQLAlchemy.in_transaction={connection.in_transaction()} "
                    f"| sqlite3.in_transaction={driver.in_transaction} | SQL={statements!r}"
                )
                statements.clear()

            checkpoint("Initial connection")
            if connection.in_transaction() or driver.in_transaction:
                raise RuntimeError("Expected an idle connection before Connection.begin()")

            outer = connection.begin()
            checkpoint("Connection.begin()")
            with Session(bind=connection, join_transaction_mode="create_savepoint") as session:
                session.connection()  # Acquire the connection and its session savepoint.
                checkpoint("Session.connection()")
                session.execute(text(INSERT))
                checkpoint("Session.execute()")
                session.commit()
                checkpoint("Session.commit()")
            checkpoint("Session.close()")
            outer.rollback()
            checkpoint("Outer rollback()")

            with closing(sqlite3.connect(path, autocommit=legacy)) as reader:
                read_statements = []
                reader.set_trace_callback(read_statements.append)
                rows = reader.execute(READ).fetchall()
                print(
                    f"New connection | sqlite3.in_transaction={reader.in_transaction} "
                    f"| SQL={read_statements!r} | rows={rows!r}"
                )
    finally:
        engine.dispose()
    return rows


def main() -> None:
    if not hasattr(sqlite3, "LEGACY_TRANSACTION_CONTROL"):
        raise SystemExit("Python 3.12 or later is required to select sqlite3 legacy transaction control")
    print(
        f"Python {platform.python_version()} | SQLAlchemy {sqlalchemy.__version__} "
        f"| SQLite {sqlite3.sqlite_version}"
    )
    with TemporaryDirectory(prefix="txnstage-savepoint-") as directory:
        legacy_rows = run_case(Path(directory) / "legacy.sqlite3", explicit_begin=False)
        control_rows = run_case(Path(directory) / "explicit-begin.sqlite3", explicit_begin=True)

    if legacy_rows != [(1, "baseline"), (2, "session")] or control_rows != [(1, "baseline")]:
        raise SystemExit("Observed rows differ from the expected legacy and explicit BEGIN outcomes")


if __name__ == "__main__":
    main()
