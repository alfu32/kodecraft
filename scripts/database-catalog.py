#!/usr/bin/env python3
"""Freeze the reference's data-only JDBC registry; never inspect user config."""
import argparse
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def registry(source):
    text = (source / "src/main/kotlin/editor/database/driver/JdbcDrivers.kt").read_text()
    section = text.split("fun defaultDefinitions(): List<JdbcDriverDefinition> = listOf(", 1)[1].split("private fun maven(", 1)[0]
    definitions = []
    for line in section.splitlines():
        match = re.match(r'\s*(maven|vendor|alias|custom)\(', line)
        if not match:
            if 'id = "elasticsearch"' in line:
                definitions.append(dict(id="elasticsearch", displayName="Elasticsearch SQL JDBC", databaseFamily="Elasticsearch",
                    driverClass="org.elasticsearch.xpack.sql.jdbc.EsDriver", defaultJdbcUrlTemplate="jdbc:es://localhost:9200",
                    urlPrefixes=["jdbc:es:"], artifact=dict(repositoryType="MAVEN_CUSTOM", groupId="org.elasticsearch.plugin",
                    artifactId="x-pack-sql-jdbc", repository="https://artifacts.elastic.co/maven")))
            elif 'id = "custom-jdbc"' in line:
                definitions.append(dict(id="custom-jdbc", displayName="Custom JDBC", databaseFamily="Custom", driverClass="",
                    defaultJdbcUrlTemplate="jdbc:", urlPrefixes=["jdbc:"]))
            continue
        values = [json.loads(value) for value in re.findall(r'"(?:[^"\\]|\\.)*"', line)]
        kind = match.group(1)
        if kind == "maven":
            identifier, name, family, coordinates, driver_class, prefix, url, *classifier = values
            group, artifact = coordinates.split(":", 1)
            artifact = dict(repositoryType="MAVEN_CENTRAL", groupId=group, artifactId=artifact)
            if classifier:
                artifact["classifier"] = classifier[0]
            value = dict(id=identifier, displayName=name, databaseFamily=family, driverClass=driver_class,
                         defaultMavenCoordinates=coordinates, defaultJdbcUrlTemplate=url, urlPrefixes=[prefix], artifact=artifact)
        elif kind == "vendor":
            identifier, name, family, driver_class, prefix, page = values
            value = dict(id=identifier, displayName=name, databaseFamily=family, driverClass=driver_class,
                defaultJdbcUrlTemplate=prefix, urlPrefixes=[prefix], artifact=dict(repositoryType="VENDOR_URL", vendorDownloadPage=page))
        elif kind == "alias":
            identifier, name, family, provider, driver_class, prefix = values
            value = dict(id=identifier, displayName=name, databaseFamily=family, driverClass=driver_class,
                         defaultJdbcUrlTemplate=prefix, urlPrefixes=[prefix], aliasOf=provider)
        else:
            identifier, name, prefix = values
            value = dict(id=identifier, displayName=name, databaseFamily=name, driverClass="",
                         defaultJdbcUrlTemplate=prefix, urlPrefixes=[prefix])
        definitions.append(value)
    assert len(definitions) == len(set(value["id"] for value in definitions))
    return dict(version=1, drivers=definitions)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, default=ROOT.parent / "kode")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    target = ROOT / "resources/database"
    data = {
        "drivers.json": json.dumps(registry(args.source), indent=2, ensure_ascii=False) + "\n",
        "jdbc-url-templates.json": (args.source / "src/main/resources/database-jdbc-url-templates.json").read_text(),
    }
    if args.check:
        assert all((target / name).read_text() == value for name, value in data.items()), "database catalog differs from frozen reference"
    else:
        target.mkdir(parents=True, exist_ok=True)
        for name, value in data.items():
            (target / name).write_text(value)
    print(f"Verified {len(json.loads(data['drivers.json'])['drivers'])} JDBC registry entries and exact URL catalog")


if __name__ == "__main__":
    main()
