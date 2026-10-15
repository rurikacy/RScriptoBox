@echo off
chcp 65001 >nul
echo first=[%~1]
echo second=[%~2]
echo third=[%~3]
echo fourth=[%~4]
echo env=[%RSCRIPTO_TEST%]
echo stderr-visible 1>&2
exit /b 0
