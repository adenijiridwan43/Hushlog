//! Documentation for the `hushlog` command-line tool.
//!
//! The executable is the `hushlog` binary target. This library target exists
//! so documentation builders have a library to render; the implementation of
//! the pipeline lives in the `hushlog-core`, `hushlog-rules` and
//! `hushlog-render` crates, each with its own API documentation.
//!
//! `hushlog` reads a log stream one line at a time, classifies each line,
//! folds repeated shapes into counted notices and renders a short result.
//! Nothing is silently dropped: `--tee` keeps the complete byte-exact input,
//! `--explain` annotates the decisions, and the end-of-run summary accounts
//! for every line.
