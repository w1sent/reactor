# Recipes

Verified against lnav 0.14.0. Swap in whatever table and column names step 1
and 2 of the main skill actually found.

## Rank a field by frequency

```
;SELECT c_ip, count(*) AS hits FROM access_log GROUP BY c_ip ORDER BY hits DESC
```

Or, for a quick look without writing the `GROUP BY` yourself:

```
:summarize c_ip
```

`:summarize` adds min/max/average/median/stddev for free when the column is
numeric; for a text column like this it is just the frequency count.

## Tag lines matching a condition

Tags are additive and queryable afterwards -- useful for marking lines of
interest across a large file before pulling out just those:

```
;UPDATE access_log SET log_tags = json_array('#client-error') WHERE sc_status >= 400 AND sc_status < 500
;SELECT log_line, sc_status, log_tags FROM access_log WHERE log_tags IS NOT NULL
```

## Correlate across every loaded file and format at once

`all_logs` is the union of every parsed line regardless of which format
matched it, with `log_format` naming the match -- reach for it when
correlating events across multiple log types (an application log and the web
server's access log, say) rather than querying one format table at a time:

```
lnav -n -N -c ';SELECT log_time, log_format, log_body FROM all_logs ORDER BY log_time' app.log access.log
```

## A reusable multi-step analysis

Put it in a script and run with `-f` -- everything from the main skill's
steps 2 and 3 works unchanged inside one. Build these against a recognized
format's real columns, not `logline`'s discovered ones (see the main skill's
step 2 -- discovered-field `WHERE`/`GROUP BY` is unreliable, so a script that
re-runs one on a schedule would just re-run the wrong answer):

```
# top-errors.lnav
;SELECT c_ip, count(*) AS errors FROM access_log WHERE sc_status >= 400 GROUP BY c_ip ORDER BY errors DESC
:write-json-to /dev/stdout
```

```
lnav -n -N -f top-errors.lnav access.log
```
