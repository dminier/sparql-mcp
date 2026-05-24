PREFIX smc: <https://sparql-mcp.dev/ns#>
INSERT { GRAPH <urn:meta> { ?p smc:description ?label } }
WHERE  { GRAPH <urn:meta> {
    ?p a smc:Project ; smc:label ?label .
    FILTER NOT EXISTS { ?p smc:description ?_d }
} }
