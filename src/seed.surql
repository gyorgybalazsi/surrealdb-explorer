-- Sample data for surrealdb-explore: a few people with `knows` relations,
-- and a company two of them link to. Loaded by "Load sample data" in the app.
-- Safe to run repeatedly: records are upserted and relations are recreated.
-- It deletes all existing `knows` relations before adding these.

UPSERT person:tobie MERGE { name: { first: 'Tobie', last: 'Morgan Hitchcock' }, title: 'Founder & CEO', marketing: true };
UPSERT person:alice MERGE { name: { first: 'Alice', last: 'Smith' }, title: 'CTO',      marketing: false };
UPSERT person:bob   MERGE { name: { first: 'Bob',   last: 'Jones' }, title: 'Engineer', marketing: false };
UPSERT person:carol MERGE { name: { first: 'Carol', last: 'White' }, title: 'Designer', marketing: true };
UPSERT person:dave  MERGE { name: { first: 'Dave',  last: 'Brown' }, title: 'Sales',    marketing: true };
UPSERT person:jaime MERGE { name: { first: 'Jaime', last: 'Lee' },   title: 'Engineer' };

-- A second table, connected through a record-link field rather than RELATE.
UPSERT company:surrealdb MERGE { name: 'SurrealDB', city: 'London' };
UPSERT person:tobie MERGE { company: company:surrealdb };
UPSERT person:jaime MERGE { company: company:surrealdb };

DEFINE TABLE IF NOT EXISTS knows TYPE RELATION;
DELETE knows;
RELATE person:tobie->knows->person:alice;
RELATE person:alice->knows->person:bob;
RELATE person:alice->knows->person:carol;
RELATE person:bob->knows->person:dave;
RELATE person:carol->knows->person:dave;
RELATE person:dave->knows->person:jaime;
RELATE person:jaime->knows->person:alice;
