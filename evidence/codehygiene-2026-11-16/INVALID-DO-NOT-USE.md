# RESULTADO INVÁLIDO — NÃO USAR SCORES OU CONTAGENS

A primeira execução concluiu com código 0, mas emitiu avisos numéricos e produziu scores NaN. As contagens de sinais desta pasta não são evidência válida de qualidade do código. A pasta é mantida somente para rastrear o diagnóstico; não é o resultado final da revisão.

No runtime NumPy 2.3.5 utilizado inicialmente, uma multiplicação de matrizes de teste divergiu da referência `einsum` e amostras pequenas do modelo produziram valores não finitos. Não foi estabelecida a causa subjacente, nem se trata de afirmação sobre todas as instalações de NumPy 2.x ou sobre defeito no modelo upstream.

Uma venv isolada com NumPy 1.26.4 passou no mesmo teste de matrizes e produziu scores finitos. O runner agora recusa resultados não finitos e JSON NaN/Infinity.

**Resultado substituto:** [resumo válido](../codehygiene-2026-11-16-numpy126/summary.json), [manifesto de escopo e versões](../codehygiene-2026-11-16-numpy126/manifest.json). Não comparar scores entre esta pasta e a substituta.
