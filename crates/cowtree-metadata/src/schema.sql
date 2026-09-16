CREATE TABLE settings(singleton INTEGER PRIMARY KEY CHECK(singleton=1), limits_json TEXT NOT NULL,
 tip INTEGER NOT NULL CHECK(tip>=0), next_leaf INTEGER NOT NULL CHECK(next_leaf>0),
 next_token INTEGER NOT NULL CHECK(next_token>0));
CREATE TABLE epochs(version INTEGER PRIMARY KEY CHECK(version>=0), root TEXT NOT NULL);
CREATE TABLE leaves(id INTEGER PRIMARY KEY CHECK(id>0), next_sequence INTEGER NOT NULL DEFAULT 1 CHECK(next_sequence>0));
CREATE TABLE leases(path TEXT PRIMARY KEY, leaf INTEGER NOT NULL REFERENCES leaves(id), token INTEGER NOT NULL UNIQUE,
 origin TEXT NOT NULL, activated INTEGER NOT NULL CHECK(activated IN (0,1)));
CREATE TABLE views(leaf INTEGER NOT NULL REFERENCES leaves(id), path TEXT NOT NULL, origin TEXT NOT NULL,
 value TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>=0),
 captured INTEGER NOT NULL DEFAULT 0 CHECK(captured IN (0,1)), PRIMARY KEY(leaf,path));
CREATE TABLE uploads(leaf INTEGER NOT NULL REFERENCES leaves(id), object TEXT NOT NULL,
 ready INTEGER NOT NULL CHECK(ready IN (0,1)), generation INTEGER NOT NULL CHECK(generation>0), PRIMARY KEY(leaf,object));
CREATE TABLE proposals(leaf INTEGER NOT NULL, sequence INTEGER NOT NULL, input_hash TEXT NOT NULL,
 body TEXT, state TEXT NOT NULL CHECK(state IN ('pending','committed','aborted')),
 captured_at INTEGER NOT NULL, attempt INTEGER NOT NULL DEFAULT 0, parent INTEGER,
 root TEXT, ready INTEGER NOT NULL DEFAULT 0 CHECK(ready IN (0,1)), committed_version INTEGER, batch_id TEXT,
 PRIMARY KEY(leaf,sequence));
CREATE TABLE retained(version INTEGER PRIMARY KEY REFERENCES epochs(version));

CREATE TABLE imports(singleton INTEGER PRIMARY KEY CHECK(singleton=1), source TEXT NOT NULL,
 paths TEXT NOT NULL, root TEXT NOT NULL, ready INTEGER NOT NULL CHECK(ready IN (0,1)));
CREATE TABLE bindings(leaf INTEGER PRIMARY KEY REFERENCES leaves(id), path TEXT NOT NULL UNIQUE,
 device INTEGER NOT NULL, inode INTEGER NOT NULL, version INTEGER NOT NULL REFERENCES epochs(version),
 pending_version INTEGER REFERENCES epochs(version), generation INTEGER NOT NULL DEFAULT 0, plan TEXT);
